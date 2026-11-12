#!/usr/bin/env python3

import json
import os
import socket
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AGENT_JAR = ROOT / "window_bridge/build/window-bridge-agent.jar"

HARNESS_SOURCE = """
public final class WindowBridgeHarness {
    public static void main(String[] args) throws Exception {
        System.out.println("[Harness] JVM started");
        Thread.sleep(3500);
        System.out.println("[Harness] JVM finished");
    }
}
"""


def receive_registration(connection):
    stream = connection.makefile("r", encoding="utf-8")
    line = stream.readline()
    stream.close()

    if not line:
        raise RuntimeError("Client disconnected before registration")

    return json.loads(line)


def verify_registration(data, loader, expected_pid):
    expected = {
        "type": "register",
        "pid": expected_pid,
        "loader": loader,
        "role": "minecraft_client",
        "protocol": 1,
    }

    if data != expected:
        raise AssertionError(
            f"Registration mismatch:\n"
            f"expected: {expected}\n"
            f"actual:   {data}"
        )

    print(f"[OK] Registration: loader={loader}, pid={expected_pid}")


def run_loader_test(loader, harness_dir):
    print()
    print(f"========== Testing {loader} ==========")

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as server:
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("127.0.0.1", 0))
        server.listen(4)
        server.settimeout(8)

        port = server.getsockname()[1]

        env = os.environ.copy()
        env["JVM_COMBINER_WINDOW_ADDR"] = f"127.0.0.1:{port}"

        command = [
            "java",
            f"-javaagent:{AGENT_JAR}={loader}",
            "-cp",
            str(harness_dir),
            "WindowBridgeHarness",
        ]

        process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            env=env,
        )

        connections = []

        try:
            # First registration.
            first, _ = server.accept()
            connections.append(first)

            first_data = receive_registration(first)
            verify_registration(first_data, loader, process.pid)

            first.sendall(
                (
                    json.dumps({
                        "type": "registered",
                        "accepted": True,
                        "pid": process.pid,
                    }) + "\n"
                ).encode("utf-8")
            )

            print("[OK] First registration accepted")

            # Simulate a server-side disconnect.
            time.sleep(0.5)
            first.close()
            connections.remove(first)
            print("[TEST] Server closed the first connection")

            # The Java Agent should reconnect automatically.
            second, _ = server.accept()
            connections.append(second)

            second_data = receive_registration(second)
            verify_registration(second_data, loader, process.pid)

            second.sendall(
                (
                    json.dumps({
                        "type": "registered",
                        "accepted": True,
                        "pid": process.pid,
                    }) + "\n"
                ).encode("utf-8")
            )

            print("[OK] Reconnection and second registration succeeded")

            return_code = process.wait(timeout=8)
            output = process.stdout.read()

            print(output, end="")

            if return_code != 0:
                raise RuntimeError(
                    f"Java harness exited with code {return_code}"
                )

            if output.count("[WindowBridge] Registered") < 2:
                raise AssertionError(
                    "Expected at least two successful registration logs"
                )

            print(f"[PASS] {loader}")

        finally:
            for connection in connections:
                try:
                    connection.close()
                except OSError:
                    pass

            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()



def run_rejection_test(harness_dir):
    print()
    print("========== Testing registration rejection ==========")

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as server:
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("127.0.0.1", 0))
        server.listen(2)
        server.settimeout(2)

        port = server.getsockname()[1]

        env = os.environ.copy()
        env["JVM_COMBINER_WINDOW_ADDR"] = f"127.0.0.1:{port}"

        command = [
            "java",
            f"-javaagent:{AGENT_JAR}=fabric",
            "-cp",
            str(harness_dir),
            "WindowBridgeHarness",
        ]

        process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            env=env,
        )

        try:
            connection, _ = server.accept()

            with connection:
                data = receive_registration(connection)
                verify_registration(data, "fabric", process.pid)

                connection.sendall(
                    (
                        json.dumps({
                            "type": "registered",
                            "accepted": False,
                            "error": "unsupported loader",
                        }) + "\n"
                    ).encode("utf-8")
                )

            print("[TEST] Server explicitly rejected registration")

            return_code = process.wait(timeout=6)
            output = process.stdout.read()

            print(output, end="")

            if return_code != 0:
                raise RuntimeError(
                    f"Java harness exited with code {return_code}"
                )

            if "Registration rejected; stopping" not in output:
                raise AssertionError(
                    "Agent did not report registration rejection"
                )

            if "retrying in" in output:
                raise AssertionError(
                    "Agent incorrectly attempted to retry after rejection"
                )

            try:
                unexpected, _ = server.accept()
                unexpected.close()
                raise AssertionError(
                    "Agent reconnected after explicit rejection"
                )
            except socket.timeout:
                pass

            print("[PASS] Explicit rejection stops retries")

        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def main():
    if not AGENT_JAR.is_file():
        raise SystemExit(f"[STOP] Agent JAR not found: {AGENT_JAR}")

    with tempfile.TemporaryDirectory(
        prefix="window-bridge-test-"
    ) as temp_dir:
        harness_dir = Path(temp_dir)
        source = harness_dir / "WindowBridgeHarness.java"
        source.write_text(HARNESS_SOURCE, encoding="utf-8")

        print("========== Compile test harness ==========")
        subprocess.run(
            [
                "javac",
                "--release",
                "21",
                "-d",
                str(harness_dir),
                str(source),
            ],
            check=True,
        )

        for loader in ("fabric", "forge", "neoforge"):
            run_loader_test(loader, harness_dir)

        run_rejection_test(harness_dir)

    print()
    print("========== ALL TESTS PASSED ==========")


if __name__ == "__main__":
    main()
