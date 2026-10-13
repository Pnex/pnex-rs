# Laya multilingual checkpoint -> one ONNX graph + tokenizer + config (the bundle
# read by crates/pnex-decide). Offline tooling only: PneX never runs Python.
# Adapted from receptron/laya export/export_onnx.py (MIT, Copyright (c) 2026 Receptron).
# Usage: python export_onnx.py <checkpoint dir> <bundle dir>
import json
import os
import shutil
import sys

import numpy as np
import torch

model_dir, out_dir = map(os.path.abspath, sys.argv[1:3])
sys.path.insert(0, model_dir)
from rl_common import build_model  # noqa: E402
from safetensors.torch import load_file  # noqa: E402

os.makedirs(out_dir, exist_ok=True)
cfg = json.load(open(os.path.join(model_dir, "rl_agent_config.json")))
model = build_model(cfg, encoder_dir=os.path.join(model_dir, "encoder"))
# The multilingual weights are stored in bf16: exported in fp32.
state = {k: v.float() for k, v in load_file(os.path.join(model_dir, "model.safetensors")).items()}
model.load_state_dict(state, strict=True)
model = model.float().eval()
model.encoder.config.reference_compile = False


class Wrapper(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, input_ids, attention_mask, marker_pos, marker_mask, qtype):
        logits, act = self.m(input_ids, attention_mask, marker_pos, marker_mask, qtype)
        return logits, torch.softmax(act.float(), -1)


w = Wrapper(model)
B, L = 2, 40
ex = (
    torch.randint(5, 1000, (B, L)),
    torch.ones(B, L, dtype=torch.long),
    torch.tensor([[3, 9, 15, 21], [3, 9, 0, 0]]),
    torch.tensor([[True, True, True, True], [True, True, False, False]]),
    torch.tensor([0, 2]),
)
ex[1][1, 30:] = 0
out = os.path.join(out_dir, "laya.onnx")
batch, seq, opts = torch.export.Dim("batch"), torch.export.Dim("seq", min=8), torch.export.Dim("options", min=2)
prog = torch.onnx.export(
    w, ex, opset_version=18, dynamo=True, optimize=True,
    input_names=["input_ids", "attention_mask", "marker_pos", "marker_mask", "qtype"],
    output_names=["logits", "act_probs"],
    dynamic_shapes={"input_ids": {0: batch, 1: seq}, "attention_mask": {0: batch, 1: seq},
                    "marker_pos": {0: batch, 1: opts}, "marker_mask": {0: batch, 1: opts}, "qtype": {0: batch}},
)
prog.save(out, external_data=True)
shutil.copytree(os.path.join(model_dir, "tokenizer"), os.path.join(out_dir, "tokenizer"), dirs_exist_ok=True)
json.dump({k: cfg[k] for k in ("max_len", "head_max_len", "temperature", "temperature_by_options")},
          open(os.path.join(out_dir, "laya_config.json"), "w"), indent=1)

import onnxruntime as ort  # noqa: E402

with torch.no_grad():
    ref_logits, _ = w(*ex)
sess = ort.InferenceSession(out, providers=["CPUExecutionProvider"])
o = sess.run(None, {"input_ids": ex[0].numpy(), "attention_mask": ex[1].numpy(), "marker_pos": ex[2].numpy(),
                    "marker_mask": ex[3].numpy(), "qtype": ex[4].numpy()})
diff = float(np.abs(o[0] - ref_logits.numpy()).max())
print("max |dlogits| torch vs onnx =", diff)
sys.exit(0 if diff < 1e-3 else 1)
