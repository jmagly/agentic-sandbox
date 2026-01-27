#!/usr/bin/env python3
"""Simple script to send a command to an agent via gRPC"""

import sys
import grpc
sys.path.insert(0, 'agent/proto')
import agent_pb2
import agent_pb2_grpc

def main():
    channel = grpc.insecure_channel('127.0.0.1:8120')
    stub = agent_pb2_grpc.AgentServiceStub(channel)

    request = agent_pb2.ExecRequest(
        agent_id='test-agent',
        command='/home/roctinam/dev/agentic-sandbox/test-script.sh',
        args=[],
        working_dir='/home/roctinam/dev/agentic-sandbox',
        timeout_seconds=300,  # 5 minutes
    )

    print("Sending command to agent...")
    try:
        for output in stub.Exec(request):
            if output.data:
                text = output.data.decode('utf-8', errors='replace')
                stream = "stdout" if output.stream == 1 else "stderr" if output.stream == 2 else "unknown"
                print(f"[{stream}] {text}", end='')
            if output.complete:
                print(f"\n--- Command complete: exit_code={output.exit_code} ---")
                if output.error:
                    print(f"Error: {output.error}")
                break
    except grpc.RpcError as e:
        print(f"gRPC error: {e.code()} - {e.details()}")

if __name__ == '__main__':
    main()
