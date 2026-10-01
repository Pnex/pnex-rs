// Push values to the local PNeX edge agent (Node >= 18, Deno or Bun: global fetch).
const AGENT = process.env.PNEX_AGENT_URL ?? "http://127.0.0.1:7070";

type Point = { key: string; value: unknown; unit?: string; ts?: number | string; record?: boolean };

export async function push(points: Point | Point[] | Record<string, unknown>, retries = 5): Promise<number> {
  for (let attempt = 0; attempt < retries; attempt++) {
    const res = await fetch(`${AGENT}/v1/points`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(points),
    });
    if (res.status === 202) return (await res.json()).accepted;
    if (res.status !== 503) throw new Error(`${res.status}: ${await res.text()}`);
    const wait = Number(res.headers.get("retry-after") ?? 1) * 1000 * (attempt + 1);
    await new Promise((r) => setTimeout(r, wait));
  }
  throw new Error("agent saturated");
}

await push({ key: "temperature", value: 21.5, unit: "°C" });
await push([
  { key: "power", value: 1520, unit: "W" },
  { key: "door_open", value: false },
  { key: "last_job", value: { id: "J-42", status: "done" }, record: true },
]);
// Flat object: every field becomes a point.
await push({ humidity: 48, pressure_hpa: 1013.2 });
console.log("ok");
