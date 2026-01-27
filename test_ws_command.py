#!/usr/bin/env python3
"""Send command via WebSocket"""

import asyncio
import json
import websockets

async def main():
    uri = "ws://127.0.0.1:8121"

    async with websockets.connect(uri) as ws:
        # Subscribe to all agent output
        await ws.send(json.dumps({"type": "subscribe", "agent_id": "*"}))
        print(f"Sent subscribe, waiting for response...")

        response = await asyncio.wait_for(ws.recv(), timeout=5)
        print(f"Subscribe response: {response}")

        # List agents
        await ws.send(json.dumps({"type": "list_agents"}))
        print(f"Sent list_agents, waiting for response...")

        response = await asyncio.wait_for(ws.recv(), timeout=5)
        print(f"Agents: {response}")

        # Send command to test-agent
        cmd = {
            "type": "send_command",
            "agent_id": "test-agent",
            "command": "/home/roctinam/dev/agentic-sandbox/test-script.sh",
            "args": []
        }
        await ws.send(json.dumps(cmd))
        print(f"Sent command, waiting for response...")

        response = await asyncio.wait_for(ws.recv(), timeout=5)
        print(f"Command response: {response}")

        # Listen for output for 35 seconds (should see 3 prints)
        print("\nListening for output (35 seconds)...")
        end_time = asyncio.get_event_loop().time() + 35
        while asyncio.get_event_loop().time() < end_time:
            try:
                msg = await asyncio.wait_for(ws.recv(), timeout=5)
                data = json.loads(msg)
                if data.get("type") == "output":
                    print(f"[{data.get('stream')}] {data.get('data')}", end='')
                else:
                    print(f"Message: {msg}")
            except asyncio.TimeoutError:
                print(".", end='', flush=True)

if __name__ == '__main__':
    asyncio.run(main())
