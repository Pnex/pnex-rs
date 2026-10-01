"""Push values to the local PNeX edge agent (Python stdlib only)."""
import json
import time
import urllib.error
import urllib.request

AGENT = "http://127.0.0.1:7070"


def push(points, retries=5):
    """POST one point (dict) or a list of points; retries on 503 backpressure."""
    body = json.dumps(points).encode()
    for attempt in range(retries):
        req = urllib.request.Request(
            f"{AGENT}/v1/points", body, {"content-type": "application/json"}
        )
        try:
            with urllib.request.urlopen(req, timeout=10) as res:
                return json.load(res)
        except urllib.error.HTTPError as err:
            if err.code != 503:
                raise RuntimeError(err.read().decode()) from err
            time.sleep(float(err.headers.get("retry-after", 1)) * (attempt + 1))
    raise RuntimeError("agent saturated")


if __name__ == "__main__":
    push({"key": "temperature", "value": 21.5, "unit": "°C"})
    push({"key": "machine_state", "value": {"mode": "auto", "alarms": []}, "record": True})
    push([
        {"key": "power", "value": 1520, "unit": "W"},
        {"key": "door_open", "value": False},
        {"key": "energy", "value": 12.4, "unit": "kWh", "ts": int(time.time() * 1000)},
    ])
    print("ok")
