#!/usr/bin/env python3
"""MatrixRTC call authorization end to end on one demo server: a fresh user
registers, creates a room, checks /.well-known/matrix/client advertises an RTC
focus, gets an OpenID token, and exchanges it at lk-jwt-service /sfu/get for a
LiveKit JWT. Exits non-zero on any missing step; an empty JWT is a failure.

usage: demo-calls-e2e.py [--server http://127.0.0.1:8092] [--user calltest-<time>]
                         [--jwt-service URL]   # default: the advertised focus
"""
import argparse, importlib.util, pathlib, secrets, sys, time

_spec = importlib.util.spec_from_file_location(
    "concierge_e2e", pathlib.Path(__file__).with_name("demo-concierge-e2e.py"))
_e2e = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(_e2e)
call, log, register, matrix_login = _e2e.call, _e2e.log, _e2e.register, _e2e.matrix_login

RTC_FOCI_KEYS = ("org.matrix.msc4143.rtc_foci", "m.rtc_foci")

def advertised_focus(origin):
    status, body = call("GET", f"{origin}/.well-known/matrix/client")
    for key in RTC_FOCI_KEYS:
        for focus in body.get(key, []):
            if focus.get("type") == "livekit" and focus.get("livekit_service_url"):
                return focus["livekit_service_url"]
    return None

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--server", default="http://127.0.0.1:8092")
    p.add_argument("--user", default=f"calltest{int(time.time())}")
    p.add_argument("--jwt-service")
    a = p.parse_args()
    focus = advertised_focus(a.server)
    log(f"well-known rtc focus: {focus}")
    jwt_service = a.jwt_service or focus
    if not jwt_service:
        sys.exit("no livekit focus advertised and no --jwt-service given")
    password = secrets.token_urlsafe(18)
    register(a.server, a.user, password)
    token, user_id = matrix_login(a.server, a.user, password)
    status, room = call("POST", f"{a.server}/_matrix/client/v3/createRoom", {"name": "call test"}, token)
    if "room_id" not in room:
        sys.exit(f"createRoom: {status} {room}")
    status, openid = call("POST", f"{a.server}/_matrix/client/v3/user/{user_id}/openid/request_token", {}, token)
    if "access_token" not in openid:
        sys.exit(f"openid: {status} {openid}")
    log(f"openid token for {openid.get('matrix_server_name')}")
    body = {"room": room["room_id"], "openid_token": openid, "device_id": "CALLTEST"}
    status, sfu = call("POST", f"{jwt_service.rstrip('/')}/sfu/get", body)
    if status != 200 or not sfu.get("jwt") or not sfu.get("url"):
        sys.exit(f"/sfu/get: {status} {sfu}")
    log(f"/sfu/get 200: LiveKit url {sfu['url']}, jwt {len(sfu['jwt'])} chars")

if __name__ == "__main__":
    main()
