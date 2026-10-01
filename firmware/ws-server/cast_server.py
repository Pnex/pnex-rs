import asyncio
import websockets
from urllib.parse import urlparse, parse_qs
import logging
from random import randint
# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

async def publish_messages(websocket, interval):
    count = 0
    while True:
        r1 = randint(0,1)
        r2 = randint(0,1)
        r3 = randint(0,1)
        r4 = randint(0,1)
        message = f"ch1={r1},ch2={r2},ch3={r3},ch4={r4}"
        await websocket.send(message)
        logger.info(f"Sent message: {message}")
        count += 1
        await asyncio.sleep(interval)

async def handler(websocket, path):
    # Parse the query parameters
    query = urlparse(path).query
    params = parse_qs(query)

    token = params.get('token', [None])[0]
    pred_dev = params.get('pred_dev', [None])[0]
    device_id = params.get('device_id', [None])[0]
    metadata = params.get('metadata', [None])[0]

    logger.info(f"New connection: token={token}, pred_dev={pred_dev}, device_id={device_id}, metadata={metadata}")

    # Start the periodic message publishing task
    interval = 0.5  # interval in seconds
    publish_task = asyncio.create_task(publish_messages(websocket, interval))

    # Handle the WebSocket connection
    try:
        async for message in websocket:
            logger.info(f"Received message: {message}")
            await websocket.send(f"Received: {message}")
    except websockets.exceptions.ConnectionClosed as e:
        logger.info(f"Connection closed: {e}")
    finally:
        publish_task.cancel()  # Cancel the publishing task when connection is closed

async def main():
    logger.info("Starting WebSocket server on ws://localhost:8765")
    async with websockets.serve(handler, "0.0.0.0", 80):
        try:
            await asyncio.Future()  # run forever
        except KeyboardInterrupt:
            logger.info("Shutting down WebSocket server")

if __name__ == "__main__":
    asyncio.run(main())
