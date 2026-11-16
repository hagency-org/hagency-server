#!/usr/bin/env python3
"""Minimal TLS-terminating TCP proxy for the local federation spike.

Palpo always sends federation traffic to https://<server_name>, and the
embedded Hagency listener speaks plain HTTP, so each instance gets one of
these in front of it. Development only: self-signed certificate, no auth.

Usage: fed-tls-proxy.py LISTEN_PORT TARGET_PORT CERT KEY
"""
import asyncio
import ssl
import sys

BUFFER = 65536


async def pump(reader, writer):
    try:
        while True:
            data = await reader.read(BUFFER)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.IncompleteReadError, ssl.SSLError):
        # The peer closed abruptly; the other direction is torn down below.
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass  # closing an already-closed TLS stream raises; nothing to do


async def handle(client_reader, client_writer, target_port):
    try:
        upstream_reader, upstream_writer = await asyncio.open_connection(
            "127.0.0.1", target_port
        )
    except OSError as error:
        print(f"upstream 127.0.0.1:{target_port} unreachable: {error}", flush=True)
        client_writer.close()
        return
    await asyncio.gather(
        pump(client_reader, upstream_writer), pump(upstream_reader, client_writer)
    )


async def main(listen_port, target_port, cert, key):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    server = await asyncio.start_server(
        lambda r, w: handle(r, w, target_port),
        host="localhost",
        port=listen_port,
        ssl=context,
    )
    bound = ", ".join(str(s.getsockname()[:2]) for s in server.sockets)
    print(f"tls proxy {bound} -> 127.0.0.1:{target_port}", flush=True)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    asyncio.run(main(int(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4]))
