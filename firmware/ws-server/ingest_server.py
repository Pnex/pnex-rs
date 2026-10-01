import asyncio
import websockets
from urllib.parse import urlparse, parse_qs
import logging

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

async def handler(websocket, path):
    # Parse the query parameters
    query = urlparse(path).query
    params = parse_qs(query)

    token = params.get('token', [None])[0]
    pred_dev = params.get('pred_dev', [None])[0]
    device_id = params.get('device_id', [None])[0]
    metadata = params.get('metadata', [None])[0]

    logger.info(f"New connection: token={token}, pred_dev={pred_dev}, device_id={device_id}, metadata={metadata}")

    try:
        async for message in websocket:
            logger.info(f"Received message: {message}")
            await websocket.send("ok")
            logger.info("Sent response: ok")
    except websockets.ConnectionClosed as e:
        logger.info(f"Connection closed: {e}")

async def main():
    logger.info("Starting WebSocket server on ws://0.0.0.0:80")
    async with websockets.serve(handler, "0.0.0.0", 80):
        try:
            await asyncio.Future()  # run forever
        except KeyboardInterrupt:
            logger.info("Shutting down WebSocket server")

if __name__ == "__main__":
    asyncio.run(main())
