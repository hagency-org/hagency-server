# Time limits of Agent execution

- Date: 2026-11-26
- Status: accepted. The last section says which rule is implemented where and
  which is still open.
- Scope: `hagency-server` (Agent service) and the owner daemon of Hagency
  Desktop. Desktop's ADR 0016 points here for the rules and records the
  daemon's side.

## Why this exists

Every authority the server hands to an owner's computer ends by itself, so
that a computer which is switched off, signed out or taken away cannot keep
acting. Those ends grew one at a time, each with a number of its own, and three
production failures came from the way they met:

1. A computer that was busy for about 25 seconds lost every Agent, because a
   proof of sign-in was trusted for 30 seconds and every lease was cut to it.
2. After the proof was lengthened to two minutes, the daemon made a new local
   session for every call, reached its limit of sixteen, and refused its own
   calls.
3. One such refusal ended a process that had just accepted a message. Nothing
   offered the message again, and its sender was told it would not be retried.

None of these was a wrong number. Each was a rule that nobody had written
down. This document is that rule set. A change to a time limit is a change to
this document first.

## The rules

### 1. Two clocks, and what each is for

Everything the server gives a computer is either **held** or **offered**.

**Held** is what cannot be given again once lost. There are exactly three
such things: the session, an Agent's execution lease, and the activity lease
of a process that is working on a message. Losing one of these while a run is
under way leaves a run whose end nobody knows.

**Offered** is everything else: a command to start a process, every ticket, a
command, challenge or grant that a poll hands out together with the lease
that keeps other polls from it, and work that has not started. Missing an
offer costs one more delivery or one more attempt, and nothing else.

The two have different clocks:

| | Clock | Ends at | When it is missed |
|---|---|---|---|
| Held | the **authorization window**: how long the server trusts a proof of sign-in without hearing from the computer again. 30 seconds by default, up to two minutes when the host is configured for it and the client asks | `least(session end, now + session's window)` | the run is unknown; nothing is run again |
| Offered | the **claim window**: a fixed 30 seconds | `now + 30 s`; a ticket is also cut to the session's end | it is offered again (rule 3) |

Consequences that are intended:

- A computer that is silent for less than the authorization window loses
  nothing it holds. This is the only purpose of a longer window.
- A longer window makes no offer slower to come back: a start that failed is
  tried again after the claim window, whatever the authorization window is.
- A computer that is gone is noticed after at most one authorization window.
  The window is therefore also the longest time a revocation at the identity
  service can go unnoticed. A sign-out or a device revocation on this server
  takes effect at once.
- The commands that start a process (the first one of a run and each later
  attempt) are long-lived records with an end written into them, and every
  later command quotes the first. Because they are offers, their
  `expires − issued` never exceeds 30 seconds, and a binary that predates a
  longer authorization window can read every command a later binary wrote.
  The command that reserves an Agent in a pool is not an offer of this kind:
  it ends with the reservation (the pool's period or the end of the
  delegation) and no rule here changes it.

### 2. A session's end only grows

A renewal never makes a session end sooner. A proof that would end earlier
than one already accepted (an older access token that arrives late, a window
the operator lowered, a clock that was set back) leaves the session as it is,
and the session runs out by itself.

Ending a session early is a revocation and nothing else. Agents are fenced
when a session, device or user is revoked, never because a number became
smaller.

A session never ends later than a proof that was accepted for it allowed:
its end is the latest `min(access token expiry, time of proof + window)` among
those proofs, so it runs at most one window past the last proof.

A renewal is answered with the end the session has, because the server cuts
every lease to that end and the computer refuses a lease that outlives the
end it was told. One reader can be outrun by this: a client that reads no
window longer than 30 seconds (Hagency Desktop before 0.5.0-rc.19) and takes
over a session that a newer client renewed cannot read the answer until the
session's end is within its reach. That is at most 90 seconds, once, and
nothing is running on that client yet.

### 3. What can be redone is redone; what cannot is never guessed

The line between the two is **acceptance**: the moment a process takes a
message as its work.

Before acceptance everything is redone:

- **A start that was not claimed, or that failed**, is reported as
  `not_started` once its claim window has ended, and the same message goes to
  a new attempt. One message has its first attempt and at most three more.
  While an attempt is being offered again, the room shows the message as
  waiting for the Agent, never as failed.
- **An answer that did not arrive** is asked for again with the exact same
  request, and the server answers a replay with what it already did.

After acceptance:

- **Work that began and whose end is unknown** is never run again by the
  server: it may have acted, or cost something. Its sender is told that the
  outcome is unknown, and decides.
- **Work that was accepted and never began** when its process ended is the one
  case this design leaves open. The owner's policy is that what is confirmed
  not to have run is run again without anyone asking. The confirmation exists:
  the server never recorded a start and refuses one after the process is
  fenced, and the computer keeps a record of each message it began. What is
  missing is a way to give the message to the next process, because one
  message has one binding to one process on both sides, and that rule is what
  protects its sender from a second answer and a second charge. Giving it a
  second binding safely means new records and a proof from both sides, and is
  a decision of its own. Until it is taken:
  - the computer does not end a process over a step that could not be made
    (rule 4), which is how this case arose in production;
  - when a process does end, the message's status says so, names the Agent,
    and its sender can send it again with one action. A message sent again is
    a new message.

### 4. A check that cannot be made is not a refusal

On the computer every failed step is one of three things, decided in one place:

| Class | Meaning | What follows |
|---|---|---|
| decided | the server, or a local rule, said no, or the step failed for a reason of its own | the work ends at once, with that reason |
| not asked | the step could not be made now (no free local session, a credential being refreshed, a busy writer) | wait and ask again, until the authority in hand ends |
| no answer | the step was sent and its outcome is not known | ask again with the exact same request |

The server answers 503 for two different things: it could not serve the
request (its database, a worker, a queue, Matrix), or its policy does not serve
the route (an operator switched pooled execution, its issuance or a retry
route off). The first is "no answer". The second is a decision: it is how an
operator drains pooled work and how a computer learns that a route is off. A
status alone does not say which, so the computer keeps the list of the
server's policy codes. A 5xx failure whose code is not on that list is "no
answer": asked again for a bounded while, never taken for a refusal that was
not made.

A local session that was replaced is "not asked" for the step, which can be
asked again under the new one; to the question "does the authority this ran
under still stand" it is the answer, and the answer is no.

Waiting never lengthens an authority and never keeps a local session between
attempts.

Known bounds: all starts on one computer share one lane, and a start that
waits holds it for at most the claim window. The daemon keeps at most 16 local
sessions, and its bridge serves at most 64 managers.

### 5. Whose clock

The server decides every end, and the computer's own wall clock decides none.
A session's end is computed by the server process from the proof, on that
process's clock, and is never later than the access token's own expiry. Every
other end is computed in the database, on the database's clock. The two clocks
are expected to be one machine's or kept in step; a server process whose clock
runs ahead can lengthen a session up to its token's expiry and no further.

The computer keeps the time that was left when an answer arrived and counts
it down on a monotonic clock from the moment the request was sent.

A lease on the computer ends when the server's lease ends. The local session
that authorizes a request bounds that request, never the run it belongs to.

### 6. Readers before writers

A longer end is first accepted by every reader (validators, stored-row checks,
the computer's store and the Agent process), and only then issued. The
operator's switch is the authorization window itself: lowering it shortens
nothing that was already given (rule 2) and takes effect as each session is
next renewed.

## Every time limit on the path of a pooled message

"Window" is the session's authorization window; "session end" is
`min(access token expiry, time of proof + window)`.

| Limit | Value | Kind | Set by |
|---|---|---|---|
| Access token life | the identity service's choice | — | identity service |
| Authorization window | 30 s by default; up to 120 s when `authorization_window_ms` is configured and the client asks with `x-hagency-authorization-window-ms` | held | host configuration and the client's request |
| Session end | see above; only grows | held | server |
| Agent execution lease | `least(session end, now + asked length)`; a length outside 1 s to 120 s is refused | held | server, from the client's request |
| Activity lease of a working process | for a session whose client follows its window (it sends `x-hagency-activity-window: session` when it presents its proof): `least(session end, now + window)`, and a renewal never ends before the lease it renews. For a session that follows none (an older client, which refuses a longer lease): `least(session end, now + 30 s)`. Never past the pool reservation it runs under | held | server |
| Start window of a start command | 30 s from issue | offered | server |
| Ticket for one exchange about a process | 30 s; the ticket that lets a process report ready also ends with its command's start window | offered | server |
| A command, challenge or grant handed out by a poll | not handed out again for 30 s (not cut to the session's end), then handed out again until it is answered | offered | server |
| Renewal of an Agent execution lease | every 5 s, or after half of what is left when that is sooner | — | computer |
| Renewal of an activity lease | when 20 s or less of it are left. Under a longer window that tolerates no longer silence than a 30 s lease does, so it becomes: 10 s after its grant, and no later than 20 s before its end, which for a lease of 20 s to 30 s is the moment it is renewed today; a lease shorter than 20 s, half-way through it (in review in Hagency Desktop; see the state table) | — | computer |
| Token refresh | as soon as a proof has lost 10 s of the window, at most once a minute for that reason; and always when a proof has 20 s or less left | — | computer |
| Local session of the daemon | 60 s, reused while 30 s are left, at most 16 at a time | bounds a request | computer |
| Grace before a silent process is given up | 180 s after its last evidence | recovery | server |
| Attempts for one message | the first and at most 3 more | recovery | server |
| How long a message waits for an Agent | 24 h | recovery | server |

What must stay ordered:

| Must hold | Why | Where it is enforced |
|---|---|---|
| 30 s ≤ window ≤ 120 s | shorter loses Agents on a busy computer; longer delays noticing a revocation | a host configuration outside the range is refused |
| access token life > window | otherwise a full proof never exists | the server only sees each token's expiry and cuts the session to it; the computer's refresh rule keeps the lead near the window |
| A held lease is renewed while at least two more renewals fit before its end | two lost renewals in a row must not lose a lease | the Agent execution lease is renewed every 5 s, which meets it under every window. The activity lease meets it once the computer renews it 10 s after its grant (in review in Hagency Desktop); renewed only with 20 s left, a lease longer than 30 s does not |
| claim window ≤ window | an offer must not outlive what it would become | the claim window equals the smallest window; it is a literal in several server files, not yet one constant |

Clocks that are not part of this design, because no computer's authority
depends on them: the server's own worker leases (among them the 30-second
lease its send loop takes to post an answer to the room), the freshness of
Matrix membership facts (30 s), the retention of configuration reports and of
renewal offers (7 days).

## State of the implementation

| Rule | State |
|---|---|
| 2. A session's end only grows; fencing only on revocation | implemented with this document (migration `2026-11-26-000155`) |
| 1. Held follows the window | the session and Agent execution leases do. The server issues an activity lease as long as the window to a session that follows it. The computer taking such a lease, holding it to its end and renewing it early enough for the length to matter is in review in Hagency Desktop; until then every session follows none and every activity lease is 30 s |
| 1. Offered keeps 30 s | holds today for every start command, ticket and poll |
| 3. A failed start is tried again | implemented (demand retries) |
| 3. Accepted work that never began is run again | open decision, see rule 3. Today it is not run again, its status has no reason, and the room says only that the message was not processed |
| 4. Three classes of failure on the computer | in review in Hagency Desktop |
| 5. A lease ends when the server's lease ends | in review in Hagency Desktop |
