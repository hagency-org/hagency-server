# Demo calls (Element Call + LiveKit)

`../../../hagency-demo-up.sh` starts this stack by default (`HAGENCY_DEMO_CALLS=0`
skips it) and `--stop` takes it down. Manual: `docker compose up -d --wait` here.

| service            | URL                        | role                                   |
|--------------------|----------------------------|----------------------------------------|
| livekit            | ws://127.0.0.1:7880        | SFU (embedded TURN on udp 3478)        |
| lk-jwt             | http://127.0.0.1:8071      | MatrixRTC auth, advertised in rtc_foci |
| element-call-alpha | http://127.0.0.1:8093      | call app for alpha (localhost:8450)    |
| element-call-beta  | http://127.0.0.1:8094      | call app for beta (localhost:8452)     |

Needs a hagency-server built with Palpo's `rtc_foci` and federation
`openid/userinfo` fixes, and alpha/beta restarted with the config
`hagency-fed-up.sh` now writes. Check: `scripts/demo-calls-e2e.py` (fresh user,
OpenID token, `/sfu/get` returns a LiveKit JWT).

## Starting a call

1:1: open the DM in the Hagency window, click the camera button in the room
header. The browser opens Element Call for that room; sign in with the same
username and password once, then "Join call". The other person does the same
from their window (or Element Web) and joins the same room's call.

Group: same, from any group room; everyone who clicks the button in that room
joins one call.

Both people on one server join directly. Across servers (ann on alpha, ben on
beta) each opens their own server's Element Call; both use the one LiveKit.
