---
id: models
title: Vision models
kind: feature
pages: /models
nodes: vision_detect, camera_source
err_codes: ml-model-write-forbidden, ml-model-missing-bytes, ml-model-load-failed, ml-model-inference-failed, ml-model-invalid
tools: 
tags: models, modèles, onnx, yolox, vision, object detection, détection, ai, ia, inference, labels, threshold, seuil
---
Models (Data › Models) is the registry of the ONNX object-detection models used by the **Object detection** node of flows. A model is an ONNX file from the media library plus its settings: family (YOLOX), input size, labels, score threshold and NMS IoU.

## What you can do
- **Add a model**: **Upload an ONNX file** or pick one **From the media library**, then set **Name**, **Family**, **Input width/height**, **Score threshold**, **NMS IoU** and **Labels** (one per line, in class order; **Reset to COCO-80** pre-fills them).
- The **Check** column shows **Valid** with the measured time per frame and the sustainable frames per second, **Error**, or **Never checked**; **Check** runs it again.
- **Test with an image**: run the model on a JPEG or PNG and see the boxes.
- **Live test**: run it on a camera's latest frames. Boxes below the threshold are dashed; move the threshold and **Apply this threshold to the model**.
- **Edit** or **Delete** a model (the ONNX file stays in the library).

## Good to know
- The file decides the input size: when the ONNX declares fixed dimensions (416 for YOLOX-nano/tiny, 640 for s/m/l), they are applied on save.
- The number of labels must match the file's classes.
- A model is loaded and run once on save; one that does not work is refused with the actual error.
- A model slower than the camera skips frames: lower the fps or use a lighter model. Live test warns about stale frames and images too dark.
- Prefer Apache-2.0 models such as YOLOX; AGPL models (YOLOv8, YOLO11) impose their license on your deployment.
- Deleting a model stops detection in the flows that use it.
