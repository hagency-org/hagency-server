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
| Held | the **authorization window**: how long the server trusts a proof of sign-in without hearing from the computer again. 30 seconds by default, up to two minutes when the host is configured for it and the client asks | never later than the session's end, and the session's end is never more than one window after its last proof | the run is unknown; nothing is run again |
| Offered | the **claim window** (`CLAIM_WINDOW_MS`): a fixed 30 seconds | `now + 30 s`, or the session's end when that is sooner. Some kinds are cut to something else (see the table of limits): a command that starts a process and the challenge that checks an Agent's Matrix membership end at `now + 30 s` or the end of the reservation they run under, and the poll hand-outs of allocation and parent commands and of replies use the plain 30 s | it is offered again (rule 3) |

Consequences that are intended:

- A computer loses nothing it holds while its silence is shorter than the
  lead it had when the silence began: the window, less the time since its
  last renewal, and never past its access token's expiry. A computer that
  renews every few seconds therefore rides out a silence of nearly one
  window. This is the only purpose of a longer window.
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

Ending a session early is a revocation and nothing else. An Agent's
reservation is fenced when something it stands on is withdrawn: its session,
device or user is revoked, its device is registered anew, or the request,
grant, pool, plan, project or room behind it ends. It is never fenced because
a number became smaller.

A fence is not undone, so it is never silent. The fence of a reservation and
the fence of a process each leave a record of when it happened, whether a rule
of the database or the service did it, and what things looked like at that
moment (`OPERATIONS.md`). For a reservation that is everything it stands on,
how long ago its reserve command, allocation and grants had ended, and, when
the service fenced it, which part of the service did. An allocation or a
grant keeps its state when its time runs out, so only those times tell a
reservation fenced for its time from one fenced for another reason. For a
process it is, among other things, how long its lease had been over.

A session never ends later than a proof that was accepted for it allowed:
its end is the latest `min(access token expiry, time of proof + window)` among
those proofs, so it runs at most one window past the last proof. It can lie
past the expiry of the token that was presented last, when an earlier token
allowed it.

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

- **A start that was not claimed, or that failed before its process
  reported**, is reported as `not_started` once its claim window has ended,
  and the same message goes to a new attempt. One message has its first
  attempt and at most three more. While an attempt is being offered again,
  the room shows the message as waiting for the Agent, never as failed.
  Three starts are not tried again today and end the message, which its
  sender can then send again (below): one whose process reported and then
  ended before it was given the message, any failed start of an Agent with an
  applied budget, and any failed start on a computer whose daemon does not ask
  for retries.
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
  - rule 4 is to keep the computer from ending a process over a step that
    could not be made, which is how this case arose in production; it is in
    review in Hagency Desktop (see the state of the implementation);
  - when a process does end, the message's status says so, names the Agent,
    and its sender can send it again with one action. A message sent again is
    a new message. The server decides who is offered that, and offers it only
    while the Agent can serve a new message: its reservation stands, by the
    same check that a new message meets when it arrives and that the polls
    make before they offer one; its executor's ledger was not replaced, it
    has not been stopped, and it is a verified member of the room. A
    reservation stands while neither it nor the intent behind it is fenced
    and its time is not over. That time is its reserve command's end; for a
    pool Agent that follows its pool's grants (a recurring one, or any one
    under grant succession), the end of the run of grants it was reserved
    under, so at the turn of a period it serves again once the next grant is
    in place; for a project Agent, also the end of its allocation and of that
    allocation's grant. An Agent with an applied budget serves too, through
    its own queue. The offer is part of the status of the reader's 16 newest
    messages in the room (their own, and every message to an Agent they
    own); an older message is not offered.

  Every message that ended without a turn ends the same way for this rule,
  whichever record ended it: work that was given to a process that never
  began it, a message that was stopped while it waited or arrived while the
  Agent was stopped, one whose start failed for good (for an Agent with an
  applied budget too), one blocked on usage without a budget (below), and
  one that waited out its 24 hours or its period. The status derives that
  end when it is read, from every record that can end a message, and writes
  nothing: a message whose time is over reads as ended before any poll has
  recorded it. One end reads one reason whichever record holds it: a
  message that waited out its time reads `event_expired`, also when the
  budget's queue recorded it (as `expired`), and so does one blocked on
  allowance once its time is over (or one that an applied budget blocked on
  usage), never the reason of its block, which says it still waits. A
  message that the earlier protocol ended keeps that end when a budget is
  applied later, also once the budget's queue has recorded an end of its
  own for it.

  A block on usage (`usage_unrepresentable`) without a budget ends the
  message at once, with that reason. The computer blocks a message so when
  it cannot account for the Agent's earlier calls in the current period (for
  an Agent without periods, in its whole life): a call whose usage
  overflowed or could not be observed, or one it cannot match to the Agent's
  assignment. Nothing clears that: neither the owner nor the sender can
  change it, and an applied budget counts the same calls. Waiting out the
  message's time would therefore change nothing, and a copy sent in the same
  period is blocked the same way, so its sender is offered to send it again
  only once the Agent's period has turned since it arrived, and never for an
  Agent without periods.

  No message holds a queue that cannot move it: a poll passes over an Agent
  it cannot offer a message to and goes on with the next Agent, and the
  budget's queue passes over a message whose room binding is gone or that
  was blocked when it arrived. Neither can be offered again, and the status
  reads its end.

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

Known bounds: the steps of a computer's worker pass run one after another, so
a start that waits for its first report holds every later step for at most the
command's start window (30 s). The daemon keeps at most 16 local sessions, and
its bridge serves at most 64 managers.

### 5. Whose clock

The end of everything is the server's. A session's end is computed by the
server process from the proof, on that process's clock, and is never later
than the expiry of a token that was accepted for it. Every other end is
computed in the database, on the database's clock. The two clocks are expected
to be one machine's or kept in step; a server process whose clock runs ahead
can lengthen a session up to its token's expiry and no further.

The computer holds an end two ways: counted from the moment it sent the
request, with the time the server put in its answer, and read against its
wall clock. The counted time does not advance while the computer sleeps; the
wall clock does, and is the second witness for exactly that. The earlier of
the two ends the lease. The wall clock may only end a lease early: it extends
none and refuses none.

Two bounds follow. A computer whose clock is ahead loses that much of every
lease, and a whole lease when it is ahead by a lease's length; that cannot be
told from having slept. And the answer that grants an Agent execution lease
carries no server time, so for that lease the wall clock is the only witness:
a computer whose clock is behind by more than a second is refused a
full-length lease. Lifting the second takes the server's time in that answer.

A lease on the computer ends when the server's lease ends. The local session
that authorizes a request bounds that request, never the run it belongs to.

### 6. Readers before writers

A longer end is first accepted by every reader (validators, stored-row checks,
the computer's store and the Agent process), and only then issued. The
operator's switch is the authorization window itself: lowering it shortens
nothing that was already given (rule 2) and takes effect as each session is
next renewed.

The open case of rule 3 follows the same order. If the owner chooses to have
accepted work that never began run again, a computer says that it takes such
a message back with `reoffers` on its demand start, poll, recover and block
requests. This server already reads that field and acts on nothing in it: a
start that carries it is stored and compared exactly as it was sent, so a
start is answered again the same way by a server that acts on the field and
by one that does not, and a start without it keeps the bytes it always had.
A server that acts on `reoffers` is therefore put into service only after
this one, and is moved back only to this release or a later one (see "Going
back").

## Every time limit on the path of a pooled message

"Window" is the session's authorization window; "session end" is
`min(access token expiry, time of proof + window)`.

| Limit | Value | Kind | Set by |
|---|---|---|---|
| Access token life | the identity service's choice | — | identity service |
| Authorization window | 30 s by default; up to 120 s when `authorization_window_ms` is configured and the client asks with `x-hagency-authorization-window-ms` | held | host configuration and the client's request |
| Session end | see above; only grows | held | server |
| Agent execution lease | `least(session end, now + asked length)`; a length outside 1 s to the host's `max_lease_ms` (120 s unless configured lower) is refused | held | server, from the client's request |
| Activity lease of a working process | `least(session end, now + window)`, and never past the pool reservation it runs under. Today `least(session end, now + 30 s)`; see the state table | held | server |
| Start window of a start command | 30 s from issue, or the end of the reservation it runs under when that is sooner (so also the challenge that checks an Agent's Matrix membership) | offered | server |
| Ticket for one exchange about a process | 30 s; the ticket that lets a process report ready also ends with its command's start window | offered | server |
| A command, challenge or grant handed out by a poll | not handed out again for 30 s or until the session's end, whichever is sooner (the hand-outs of allocation and parent commands and of replies use the plain 30 s), then handed out again until it is answered | offered | server |
| Renewal of an Agent execution lease | every 5 s, or after half of what is left when that is sooner | — | computer |
| Renewal of an activity lease | when 20 s or less of it are left. Under a longer window this has to become "when it has lost 10 s", or the longer lease tolerates no longer silence than the short one; see the state table | — | computer |
| Token refresh | as soon as a proof has lost 10 s of the window, at most once a minute for that reason; and always when a proof has 20 s or less left | — | computer |
| Local session of the daemon | 60 s, reused while 30 s are left, at most 16 at a time | bounds a request | computer |
| Grace before a silent process is given up | 180 s after the end of its last lease or start window, or after its last recorded step when that is later. The server gives such processes up in the background and in its computer's attempt polls; in both, one it cannot give up is logged and does not hold back the others | recovery | server |
| A tool request waiting for its sender's answer | as long as the computer asks, 120 s at most | bounds a request | computer, capped by the server |
| A worker's declaration of what it can run | trusted for 300 s | — | server |
| A reservation for one encryption exchange | 15 s, of which 12 s are for the exchange | offered | server |
| A waiting message at the turn of a pool's period | ended as expired (`event_expired`): a message is answered in the period it arrived in. One blocked on allowance ends the same way, and so does one that an applied budget blocked on usage. One blocked on usage without a budget has ended already (rule 3); from the turn on, its sender can send it again. The status reads this end when it is read, whether or not a poll has recorded it (none records it for a blocked message: no poll offers it again) | recovery | server |
| Attempts for one message | the first and at most 3 more | recovery | server |
| How long a message waits for an Agent | 24 h, read like the period's end: the status shows the message as expired once its time is over, whether or not a poll has recorded it. A message blocked on usage without a budget does not wait (rule 3) | recovery | server |

What must stay ordered:

| Must hold | Why | Where it is enforced |
|---|---|---|
| 30 s ≤ window ≤ 120 s | shorter loses Agents on a busy computer; longer delays noticing a revocation | a host configuration outside the range is refused |
| `max_lease_ms` ≥ window | an Agent execution lease shorter than the window gives away the tolerance the window bought | not enforced; both default so that it holds |
| access token life > window | otherwise a full proof never exists | the server only sees each token's expiry and cuts the session to it; the computer's refresh rule keeps the lead near the window |
| A held lease is renewed while at least two more renewals fit before its end | two lost renewals in a row must not lose a lease | the Agent execution lease is renewed every 5 s, which meets it under every window. The activity lease does not meet it today: it is renewed once, with 20 s left |
| claim window ≤ window | an offer must not outlive what it would become | the server's `CLAIM_WINDOW_MS` (30 s) names the claim window, and the server is not built when it is longer than the default window, the smallest a host grants. The database holds several records to the same 30 s (see below) |

Clocks that are not part of this design, because no computer's authority
depends on them: the server's own worker leases (among them the 30-second
lease its send loop takes to post an answer to the room), the freshness of
Matrix membership facts (30 s), the retention of configuration reports and of
renewal offers (7 days).

## State of the implementation

| Rule | State |
|---|---|
| 2. A session's end only grows; fencing only on withdrawal | implemented (migration `2026-11-26-000155`) |
| 1. Held follows the window | the session and Agent execution leases do. Activity leases are still 30 s; following the window, and renewing them early enough for it to matter, is in review |
| 1. Offered keeps 30 s | holds today for every start command, ticket and poll |
| 3. A failed start is tried again | implemented for a start that never reported a process (demand retries); see rule 3 for the three that are not |
| 3. Accepted work that never began is run again | open decision, see rule 3. Today it is not run again. The server reads the field by which a computer would say that it takes such a message back (`reoffers`) and acts on nothing in it (rule 6). Its status has the reason `never_started` and names the Agent. Every message that ended without a turn (this one, one that was stopped while it waited, one that arrived while the Agent was stopped, one that waited out its 24 hours or its period, one whose start failed for good, also for an Agent with an applied budget, one blocked on usage without a budget) says whether its reader can send it again (`canSendAgain`: the reader sent it, and the Agent can serve a new message, see rule 3; after a block on usage, only once the Agent's period has turned since the message arrived) and which record ended it (`endReason`, present only then: for example `never_started`, `explicit_stop`, `event_expired` (also for one blocked on allowance, or by an applied budget on usage, once its time was over), `head_changed`, `retry_exhausted` or `usage_unrepresentable`); Hagency Desktop uses `canSendAgain` to name the Agent in the room and to offer the sender "Send again" |
| 4. Three classes of failure on the computer | in review in Hagency Desktop |
| 5. A lease ends when the server's lease ends | in review in Hagency Desktop |
| 5. The wall clock only ends a lease early | holds for a proof, a ticket and a start window. In review in Hagency Desktop: its store still refuses an activity lease that reads longer than allowed by the wall clock, which a slow clock makes of every fresh lease. Open: the Agent execution lease (rule 5) |

## What this design does not do

Each of these is a bound that was looked at and left, with its reason.

- **A lapsed lease is not revived.** A process whose activity lease ran out is
  fenced, and its queue is kept for the grace. Letting the same process renew
  late was considered and refused: how long silence is tolerated is the
  window, and a second tolerance beside it would be a second mechanism with
  its own state on both sides; a turn in flight cannot be suspended, only
  ended; and a renewal that cannot observe Matrix is "no answer" on the
  computer (rule 4), asked again until the lease ends. So a stall of the
  server's Matrix observation, or of the path between computer and server,
  that outlasts the lead a lease had when the stall began ends the pooled
  processes it covers. Today that lead is about 20 s under every window: an
  activity lease is `least(session end, now + 30 s)` whatever the window,
  and the computer renews it when 20 s or less of it are left (the premise;
  a runtime that renews later has less). Once activity leases follow the
  window and are renewed when they have lost 10 s (in review), the lead
  becomes the window less about 10 s: about 110 s under a 120 s window.
  What such a process had been given and not begun is rule 3's subject.
- **An Agent with an applied budget has no further attempts.** Its protocol
  has no retry: a start that fails ends the message. Its status says so and
  its sender can send it again, as for every other message that ended
  without a turn. A message that the earlier protocol had started when the
  budget was applied ends the same way (`head_changed`), and one that it had
  already ended keeps that end: the budget's queue can take neither up, and
  offers the next message instead of waiting behind it.
- **A block on usage is not settled.** Without a budget, a message that the
  computer blocked on usage ends at once (rule 3): the computer cannot
  account for the Agent's earlier calls and keeps that until the period
  turns, for an Agent without periods for good. Only a way for the computer
  to settle such calls would let a later message run, and that is a change of
  its accounting, not of a time limit. Under an applied budget, a block on
  usage waits for a new budget, as a block on allowance does, and that Agent's
  budget queue waits behind it; since a new budget counts the same calls, the
  message ends only with its 24 hours or its period.
- **The claim window is one constant with one exception.** The tickets about
  a pooled process still spell out 30 s, because the helper that ends them
  also ends that process's activity lease, which is held and is to follow
  the window. The database holds the start window of a runtime command
  and of a pooled process's command, and the tickets about a process, a
  budget and a settlement, to 30 s as well (migrations
  `2026-11-19-000114`, `000115`, `000118`, `000122` and `000124`): a
  longer claim window needs a migration too.

## Bringing a change of the window into service

The order matters, because the parts were released at different times.

1. **The server first.** A server without migration `2026-11-26-000155`
   fences every pool Agent of a computer whose session end becomes smaller.
   On such a server the window must not be lowered, and a computer must not
   be moved back to a client that asks for a shorter one, while pool Agents
   exist. With the migration, lowering the window withdraws nothing.
2. **Then the computers.** A window above 30 s is safe only for computers
   whose daemon shares one local session among its calls (Hagency Desktop
   after 0.5.0-rc.19; that release still makes one per call and can refuse
   its own calls under a longer window). Older daemons keep working at 30 s:
   the window is only granted to a client that asks for it.
3. **Then raise the window.** It takes effect as each session is next
   renewed.

Going back:

- Lowering the window again is safe once step 1 is done.
- Moving the server back to an older binary is safe; the older binary reads
  everything a newer one wrote, because an offer never exceeds 30 seconds.
- Do not run the down migration of `2026-11-26-000155` while pool Agents
  exist: it puts back the rule that fences on a smaller end.
- The down migration of `2026-11-26-000156` removes the rules that write
  fence records and keeps the records (`management_agent_fences` and
  `management_shared_runtime_fences`); a fence made while it is down leaves
  none. Its up step can run again over the kept records and puts the rules
  back.
- The down migration of `2026-11-27-000158` puts back the reservation's
  record rule of `000156` and keeps every record; a fence made while it is
  down leaves a record without `source` and without the times that had
  ended. A server that names its fences runs on such a database unchanged:
  the earlier rule does not read the name. Its up step can run again.
- Migration `2026-11-26-000154`, which comes with activity leases that follow
  the window (in review), has a lower number than `000155` and `000156`. A
  database that already has those applies it out of version order, because
  the server applies every migration it has not recorded, whatever its
  number. That is harmless: it only replaces the CHECK constraint on the
  epochs of activity renewal rows, which neither later migration reads or
  changes.
- A server that acts on `reoffers` (rule 3, if it is chosen) goes back only
  to a release that reads the field (rule 6) or a later one. A release
  before that refuses every demand request that carries `reoffers`, and a
  computer sends a saved start again with its original bytes; to go back
  further, first let every start that said `reoffers` finish.
