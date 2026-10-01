"""High-rate producer: 8 threads pushing batches concurrently (stdlib only).

The agent group-commits concurrent requests into single disk transactions;
batches of 100–1000 points per request give the best throughput.
"""
import json
import math
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor

AGENT = "http://127.0.0.1:7070/v1/points"


def send(batch):
    req = urllib.request.Request(AGENT, json.dumps(batch).encode(), {"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as res:
        return json.load(res)["accepted"]


def batches(sensor, n_batches, size):
    for b in range(n_batches):
        now = int(time.time() * 1000)
        yield [
            {"key": f"vibration_{sensor}", "value": math.sin((b * size + i) / 10), "ts": now + i}
            for i in range(size)
        ]


if __name__ == "__main__":
    start = time.time()
    with ThreadPoolExecutor(max_workers=8) as pool:
        jobs = [pool.submit(send, batch) for s in range(8) for batch in batches(s, 20, 500)]
        total = sum(j.result() for j in jobs)
    print(f"{total} points queued in {time.time() - start:.2f} s")
