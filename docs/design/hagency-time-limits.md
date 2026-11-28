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
| Held | the **authorization window**: how long the server trusts a proof of sign-in without hearing from the computer again. 30 seconds by default, up to two minutes when the host is configured for it and the client asks in the way that says it can use one (rule 6) | never later than the session's end, and the session's end is never more than one window after its last proof | the run is unknown; nothing is run again |
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

The line between the two is **the start of the turn**: the record the server
makes, at the computer's request and before the Agent's program is given the
message, that work on it begins. The rule rests on one premise about the
computer: it starts the turn only when the answer to its request says that
the record was made now.

Before the start everything is redone:

- **A start of a process that was not claimed, or that failed before its
  process reported**, is reported as `not_started` once its claim window has
  ended, and the same message goes to a new attempt.
- **A process that ended before the turn began**, whether or not it had been
  given the message, is reported as stopped, and the same message goes to a
  new attempt, if the computer said, when it started that process, that it
  takes messages back, and the operator has this switched on (it is off
  unless configured; see "Bringing the re-offer into service" below for the
  switch and the way back). Whether a turn had begun is not taken on the
  computer's word: the server reads its own record. A start is recorded only
  for a process that holds a live lease, a fence ends the lease, the server
  makes one change at a time, and it never records a start for a message it
  took back; so, with the premise above, a fenced process whose message has
  no recorded start never began it and never will. At the fence the server
  takes the message back from that process, keeping a record of what it had
  handed out, while the Agent can serve it: by the same rule that decides
  who is offered a message sent again (below), and only while no budget
  change was applied to it. So a Stop gives nothing back: it ends the
  Agent's processes, the message a process held ends as before, and the
  waiting messages are cancelled. A message taken back is a waiting message
  again in its old place: a Stop cancels it with the others, and its time
  and its period end it as they end any waiting message. If a budget change
  is applied to the Agent first, it is no longer offered and ends as every
  message the earlier protocol had started when the budget was applied
  (see the state of the implementation): its status says so at once, and
  the budget's queue offers the next message. While a process holds a
  message that it has not begun, from a start that said it takes messages
  back, the message after it is not offered yet: if the first is taken
  back, it runs first, in the next process, and the next one follows it
  there. Offered earlier, the next message would name a process that is no
  longer the last one, and would have to end.
- One message has its first attempt and at most three more. While an attempt
  is being offered again, the room shows the message as waiting for the
  Agent, never as failed.
- **An answer that did not arrive** is asked for again with the exact same
  request, and the server answers a replay with what it already did.

A new attempt is made only when the computer asks for one with its evidence
that the earlier process stopped, and before it asks for its next message.
The server never hands a message to a computer a second time unasked: a
computer that did not ask has its own record of that message, and would
refuse it. Nor does a message wait for a computer that does not come back.
Its wait has one bound, counted once: 3 minutes after the later of the end of
the grace before its first process is given up (see the table below) and the
computer's first request to run it again. A message that is waiting for its
computer when the bound has passed ends then, and no new attempt is started
for it after that; later attempts do not move the bound. That end is one
that a write records (`retry_lapsed`): the sweep, the demand poll and the
work poll, or the computer's request for a retry records it once the bound
has passed, and a start of the retry is refused from the bound on. The status never works it out from the clock, so it is
read as soon as it is recorded, with no claim window added (an end that a
poll recorded, below); until then the message reads as waiting. Its own
time and its period end a message taken back as any waiting message, one
claim window late in the status. An attempt that
started in time and then ends without its turn ends the message at once. A
computer that does not take messages back never reads one that was taken
back: for it the message ends, as before. Three kinds of start are therefore
not tried again and end the message: any failed start of an Agent with an
applied budget, whose protocol has no retry; any process that ended before
its turn whose computer did not say that it takes messages back; and any
that a computer does not ask about in time (see the state of the
implementation for which daemons ask for what). For those, and for a
message that was taken back and then ends, the message's status says that
the Agent did not process it, names the Agent, and names the record that
ended it. One taken back reads as any other end of a waiting message of the
earlier protocol: state `blocked`, reason `head_changed`, and as the record
that ended it the reason of its block (`retry_lapsed`, `retry_exhausted`,
`retry_unsupported`, `reoffer_unsupported` or `head_changed`). Hagency
Desktop reads that as "did not process" when the status names the Agent; a
released Hagency Desktop up to rc.19 shows "Message could not start. Check
the resource and Agent connection." instead. Its sender
can send it again with one action. A message sent again is a new message.
The server decides who is offered that, and offers it only while the Agent
can serve a new message: its reservation stands, by the same check that a
new message meets when it arrives and that the polls make before they offer
one; its executor's ledger was not replaced, it has not been stopped, and it
is a verified member of the room. A reservation stands while neither it nor
the intent behind it is fenced and its time is not over. That time is its
reserve command's end; for a pool Agent that follows its pool's grants (a
recurring one, or any one under grant succession), the end of the run of
grants it was reserved under, so at the turn of a period it serves again
once the next grant is in place; for a project Agent, also the end of its
allocation and of that allocation's grant. An Agent with an applied budget
serves too, through its own queue. The offer is part of the status of the
reader's 16 newest messages in the room (their own, and every message to an
Agent they own), or in the thread when the read names one, so a thread's
read lists its own 16. Each message comes with every Agent's row of it, so
that no message is listed, nor offered, for only some of its Agents; a
message that waits for a computer that can read it (an encrypted one)
counts among the 16 as well. An older message is not offered by that read.
Hagency Desktop keeps what an earlier read offered of a message that a
later read does not list. The server takes a message
back only from an Agent that serves by this same rule, and only while no
budget was applied to it. The rule is written once, in the database
(`shared_agent_serves()`), the reservation's time included; the status also
makes the check a new message meets when it arrives, which in addition holds
the reservation to the command, the people and the device it was approved
for.

Every message that ended without a turn ends the same way for this rule,
whichever record ended it: work that was given to a process that never
began it, a message that was stopped while it waited or arrived while the
Agent was stopped, one whose start failed for good (for an Agent with an
applied budget too), one that was taken back and then ended, one blocked on
usage (below), and one that waited out its 24 hours or its period. The status derives that end when it is read, from every record that
can end a message, and writes nothing: a message whose time is over reads as
ended before any poll has recorded it, one claim window (30 s) after its
time is over. A poll binds a message only while its own clock is before
that end, and every database operation of the server commits within 30 s
of its start or is undone (`OPERATION_TIMEOUT` of `hagency-db-runtime`; the
server is not built when that is longer than the claim window). So a
message whose sender is offered to send it again is one that no poll can
still bind and run; only a commit already sent when those 30 s run out can
land a moment later. An end that a poll recorded reads at once, as nothing
binds a message after it. One end reads one reason whichever
record holds it: a message that waited out its time reads `event_expired`,
also when the budget's queue recorded it (as `expired`), and so does one
blocked on allowance once its time is over, never the reason of its block,
which says it still waits. A message that the earlier protocol ended keeps that end when a
budget is applied later, also once the budget's queue has recorded an end of
its own for it. A message taken back reads `head_changed` once a budget is
applied to its Agent, as the budget's queue then records it, and its bound
later records the same reason for its computer.

A block on usage (`usage_unrepresentable`) ends the message at once, with
or without an applied budget. The computer blocks a
message so when it cannot account for the Agent's earlier calls in the
current period (for an Agent without periods, in its whole life): a call
whose usage overflowed or could not be observed, or one it cannot match to
the Agent's assignment. Nothing clears that: neither the owner nor the
sender can change it, and an applied budget counts the same calls, so a new
budget does not either. Waiting out the message's time would therefore
change nothing, and a copy sent in the same period is blocked the same way,
so its sender is offered to send it again only once the Agent's period has
turned since it arrived, and never for an Agent without periods.

A block on allowance under an applied budget (`budget_blocked`) is lifted
by a new budget, and the message waits for one until its time is over.
A copy sent while the budget that blocked it is still the one applied
would wait for the same allowance and end the same way a day later, so
its sender is offered to send it again only once another budget has been
applied, or the Agent's period has turned since it arrived.

Without an applied budget, the earlier protocol blocks a message on
allowance when the Agent's known use in its period has reached its
reservation's tokens, or when its computer reports that none of them are
left. In a period the known use only grows, and the reservation does not
change, so while that use still covers the reservation a copy would be
blocked the same way and end the same way a day later. The sender of such
a message is therefore offered to send it again only while the Agent's
known use in its current period is below its reservation's tokens (its
period has turned since, or the computer blocked it on tokens that work
still in flight held, which the server does not count), or once a budget
has been applied. A copy offered after a block on held tokens can be
blocked once more while that work holds them; once it has ended, its use
is known and read here.

Under an applied budget, the budget's queue records the end of a message
blocked on usage as `head_changed` and offers the next message. A
computer keeps only the ends `expired`, `cancelled` and `head_changed` of
that queue, and every version of Hagency Desktop keeps this one. The block
beside that end names the cause. In either lane the status reads such a
message as `cancelled`, with no `reason` and `usage_unrepresentable` as
its `endReason`: a computer's own record of the message reads
`head_changed`, while the room reads the cause. The status reads this end
as soon as the block is recorded, before the queue has recorded its end,
and keeps it when the message's time is over or its Agent is stopped
later; the database records no block of a message that has ended.
Hagency Desktop reads `endReason` first and says that the Agent's usage
could not be accounted for. Hagency Desktop 0.5.0-rc.19 reads the reason
`usage_unrepresentable` as a wait for usage to be confirmed, with nothing
that ends it, and reads the state `cancelled` as an end that will not be
tried again; the state and reason above are chosen so that it shows such
a message as ended.

No message holds a queue that cannot move it: a poll passes over an Agent it
cannot offer a message to and goes on with the next Agent, and the budget's
queue passes over a message whose room binding is gone or that was blocked
when it arrived, and ends one blocked on usage. None of these can be offered
again, and the status reads its end.

Beside its state and reason, a row of the status says:

- `endReason`: the reason of the record that ended it (above). A message
  that still waits, or waits for allowance in its period, has none, and so
  has one taken back while it waits to be offered again.
- `canSendAgain`: true when the reader sent the message, a copy sent now
  could run (after a block on usage or on allowance, as above), its Agent
  can serve a new message, and the row names the Agent by its Matrix ID
  (`agentMxid`). The client sends the copy to the Agent the row names, so
  a row without that ID offers nothing.
- `notOfferedBecause`: present only on a row of the reader's own message
  that ended without a turn and whose copy could run, when nothing but
  its Agent keeps it from being offered; `canSendAgain` is then false. Its
  value is `agent_unnamed` when the row has no Matrix ID for the Agent;
  otherwise `agent_stopped` when the Agent does not run (it was stopped,
  or waits to be started); otherwise `agent_unavailable` (its assignment
  ended, its reservation does not stand, its executor's ledger was
  replaced, or it is not a verified member of the room). A row without
  the field is offered, or is not offered for the message itself (it has
  not ended, or a copy could not run), or is read by someone who did not
  send the message. The field says no more than that: whether an Agent is
  available is the server's decision, not worked out by a client from the
  facts behind it, and which check failed stays with the Agent's owner.
- `viewerCanReview`: present only on a row whose outcome is unknown
  (`unknown` or `reply_unknown`). True when the reader manages the Agent,
  or administers its project or its pool: the rule by which the review of
  original task records lists an Agent (with pool Agents included, as
  Hagency Desktop asks for them). A sender who does neither reads false,
  as the review would list nothing for them.

After the start:

- **Work that began and whose end is unknown** is never run again by the
  server: it may have acted, or cost something. Its sender is told that the
  outcome is unknown, and decides. This includes a start whose record was
  made and whose answer never reached the computer: the computer did not
  begin the turn, and the server cannot know that.


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

The server's time in an answer is its `Date` header, in whole seconds; every
answer of this server's API carries one, and `Cache-Control: no-store`. Hagency
Desktop reads the date only from an answer that has exactly one `Date`,
`no-store` and no `Age` other than 0, so that an answer a cache may have kept
is never taken for the server's time. A proxy in front of the server must
therefore pass `Date` and `Cache-Control` through unchanged and must not
serve these answers from a cache. Without a date the computer reads a proof
on its wall clock alone and allows it no more than 31 s: a proof with a
longer window is then refused (`authorization_time_window_unavailable`), so
a longer window needs the header.

Two bounds follow. A computer whose clock is ahead loses that much of every
lease, and a whole lease when it is ahead by a lease's length; that cannot be
told from having slept. And Hagency Desktop reads the answer that grants an
Agent execution lease without its date, so for that lease the wall clock is
the only witness: a computer whose clock is behind by more than a second is
refused a full-length lease, and a proof that comes without a date. Counting
that lease from its request with the answer's date, and ending a proof on the
wall clock without refusing it, are in review in Hagency Desktop (#246); an
answer without a date still leaves the wall clock as the only witness there.

A lease on the computer ends when the server's lease ends. The local session
that authorizes a request bounds that request, never the run it belongs to.

### 6. Readers before writers

A longer end is first accepted by every reader (validators, stored-row checks,
the computer's store and the Agent process), and only then issued. The
operator's switch is the authorization window itself: lowering it shortens
nothing that was already given (rule 2) and takes effect as each session is
next renewed.

A window longer than 30 seconds is issued the same way: only to a client that
has said it can hold one. A client says so by asking for the window with the
request header `x-hagency-authorization-window-v2-ms` when it presents its
proof (`sessions/pasion` and `sessions/current/renew`). Its value is the
milliseconds asked for, and the window granted is that, at least 30 s and at
most the host's `authorization_window_ms`. By sending it, a client declares
that it shares one local session per server session, so a longer window
costs it nothing; a client that makes one local session per call is the one
that failure 2 above describes. The header that the first clients sent,
`x-hagency-authorization-window-ms`, is still accepted and gives 30 s
whatever it asks. When both are sent, the second header decides; a second
header whose value cannot be read gives 30 s, also beside a readable first
one. A client that can use a longer window sends both, so that a server from
before the second header grants it the window too.

So a computer that sends only the first header runs at 30 s, whatever window
is configured. That is Hagency Desktop 0.5.0-rc.19, which makes one local
session per call, and it is also every build that could use a longer window
but does not yet send the second header. Such a build returns to the longer
window at its first proof after it sends the second header.

The re-offer of rule 3 follows the same order. A computer says that it takes
a message back with `reoffers` on its demand start, poll, recover and block
requests. A release that reads the field and acts on nothing in it goes into
service first. It stores and compares a start that carries the field exactly
as it was sent, so the two releases answer a start again the same way, and a
start without it keeps the bytes it always had. The release that acts on it
goes back only to that release or a later one, and it has a switch of its
own (see "Bringing the re-offer into service, and back").

## Every time limit on the path of a pooled message

"Window" is the session's authorization window; "session end" is
`min(access token expiry, time of proof + window)`.

| Limit | Value | Kind | Set by |
|---|---|---|---|
| Access token life | the identity service's choice | — | identity service |
| Authorization window | 30 s by default; up to 120 s when `authorization_window_ms` is configured and the client asks with `x-hagency-authorization-window-v2-ms`. The first header, `x-hagency-authorization-window-ms`, gives 30 s whatever it asks (rule 6) | held | host configuration and the client's request |
| Session end | see above; only grows | held | server |
| Agent execution lease | `least(session end, now + asked length)`; a length outside 1 s to the host's `max_lease_ms` (120 s unless configured lower) is refused | held | server, from the client's request |
| Activity lease of a working process | `least(session end, now + 30 s)` whatever the window, and never past the pool reservation it runs under. A lease that follows the window, `least(session end, now + window)` for a session whose client asks for it, is in review (#97); see the state table | held | server |
| Start window of a start command | 30 s from issue, or the end of the reservation it runs under when that is sooner (so also the challenge that checks an Agent's Matrix membership) | offered | server |
| Ticket for one exchange about a process | 30 s; the ticket that lets a process report ready also ends with its command's start window | offered | server |
| A command, challenge or grant handed out by a poll | not handed out again for 30 s or until the session's end, whichever is sooner (the hand-outs of allocation and parent commands and of replies use the plain 30 s), then handed out again until it is answered | offered | server |
| Renewal of an Agent execution lease | every 5 s, or after half of what is left when that is sooner | — | computer |
| Renewal of an activity lease | when 20 s or less of it are left (the Claude driver: when 15 s are left); for today's 30 s lease that is 10 s after its grant. A lease as long as a longer window, renewed so, tolerates no longer silence than the 30 s one. In review in Hagency Desktop (#243, #251): renewed 10 s after its grant, no later than 20 s before its end and no sooner than 5 s after its grant, and a lease of 20 s or less half-way through, by every driver; see the state table | — | computer |
| Token refresh | as soon as a proof has lost 10 s of the window, at most once a minute for that reason; and always when a proof has 20 s or less left | — | computer |
| Local session of the daemon | 60 s, reused while 30 s are left, at most 16 at a time | bounds a request | computer |
| Grace before a silent process is given up | 180 s after the end of its last lease or start window, or after its last recorded step when that is later. The server gives such processes up in the background and in its computer's attempt polls; in both, one it cannot give up is logged and does not hold back the others | recovery | server |
| How long a message taken back from a process waits for its computer | 180 s after that grace ends for its first process, or after the computer first asked for its retry when that is later; counted once per message, so later attempts do not extend it, and an attempt started before then that ends without its turn ends the message at once | recovery | server |
| A tool request waiting for its sender's answer | as long as the computer asks, 120 s at most | bounds a request | computer, capped by the server |
| A worker's declaration of what it can run | trusted for 300 s | — | server |
| A reservation for one encryption exchange | 15 s, of which 12 s are for the exchange | offered | server |
| A waiting message at the turn of a pool's period | ended as expired (`event_expired`): a message is answered in the period it arrived in. One blocked on allowance ends the same way. One blocked on usage has ended already, with or without an applied budget (rule 3); from the turn on, its sender can send it again. The status reads this end when it is read, one claim window (30 s) after the turn, whether or not a poll has recorded it (none records it for a blocked message: no poll offers it again) | recovery | server |
| Attempts for one message | the first and at most 3 more | recovery | server |
| How long a message waits for an Agent | 24 h, fixed (not `queue.event_ttl_ms`, which is for an owner's own Agents), read like the period's end: the status shows the message as expired one claim window (30 s) after its time is over, whether or not a poll has recorded it. A message blocked on usage does not wait (rule 3) | recovery | server |

What must stay ordered:

| Must hold | Why | Where it is enforced |
|---|---|---|
| 30 s ≤ window ≤ 120 s | shorter loses Agents on a busy computer; longer delays noticing a revocation | a host configuration outside the range is refused |
| `max_lease_ms` ≥ window | an Agent execution lease shorter than the window gives away the tolerance the window bought | not enforced; both default so that it holds |
| access token life > window | otherwise a full proof never exists | the server only sees each token's expiry and cuts the session to it; the computer's refresh rule keeps the lead near the window |
| A held lease is renewed while at least two more renewals fit before its end | two lost renewals in a row must not lose a lease | the Agent execution lease is renewed every 5 s, which meets it under every window. The activity lease does not meet it today. A request from the computer is given up after 15 s, so a renewal that hangs that long, with the 2 s before it is asked again, uses 17 s of the lease: a 30 s lease renewed when 20 s are left has 3 s for one more try, and one the Claude driver renews when 15 s are left has none. A lease as long as a 120 s window (in review, #97), renewed 10 s after its grant (in review in Hagency Desktop, #243 and #251), has room for about six tries |
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
| 1. Held follows the window | the session and Agent execution leases do. Activity leases are still 30 s; following the window is in review (#97), and renewing them early enough for it to matter is in review in Hagency Desktop (#243, #251) |
| 6. A window over 30 s only for a client that says it can hold one | implemented: granted only through `x-hagency-authorization-window-v2-ms`; the first header gives 30 s |
| 1. Offered keeps 30 s | holds today for every start command, ticket and poll |
| 3. A start that was never claimed is tried again | implemented (demand retries, `shared-demand-retry-v1`) |
| 3. A process that ended before the turn began is tried again | implemented on the server (migration `2026-11-27-000159`, `shared-demand-retry-v2`), off unless the operator sets `enable_shared_demand_reoffers`, for a computer that says, in each demand start and read, that it takes messages back (`reoffers`): the message is a waiting message again at the fence, a retry is opened by the evidence that the process stopped, a message whose computer does not come back for it ends after one bound counted from its first process (`retry_lapsed`), and while a process holds a message before its turn the next one is not offered. A computer says it from the Hagency Desktop change that is in review; an earlier daemon does not, and for it the message ends as before |
| 3. A message that ended without a turn can be sent again | implemented. Every message that ended without a turn (one given to a process that never began it, one that was stopped while it waited, one that arrived while the Agent was stopped, one that waited out its 24 hours or its period, one whose start failed for good, also for an Agent with an applied budget, one taken back that then ended, and one blocked on usage) says whether its reader can send it again (`canSendAgain`: the reader sent it, a copy could run, the Agent can serve a new message, and the row names it; see rule 3), which record ended it (`endReason`, present only then: for example `never_started`, `explicit_stop`, `event_expired` (also for one blocked on allowance once its time was over), `head_changed`, `retry_exhausted`, `usage_unrepresentable` or, for one taken back, `retry_lapsed`), and, to its sender only, why it is not offered when nothing but its Agent stands in the way (`notOfferedBecause`: `agent_stopped`, `agent_unavailable` or `agent_unnamed`). A row whose outcome is unknown says whether its reader may review the Agent's original task records (`viewerCanReview`). The status lists the reader's 16 newest messages, each with every Agent's row. Hagency Desktop names the Agent in the room from the row's `agentMxid` and offers the sender "Send again" where `canSendAgain` is true |
| 4. Three classes of failure on the computer | in review in Hagency Desktop |
| 5. A lease ends when the server's lease ends | in review in Hagency Desktop |
| 5. The wall clock only ends a lease early | holds for a ticket and a start window. Not yet for the session's proof: Hagency Desktop refuses a proof that comes without a date it can read when the proof's end lies more than 31 s away on its wall clock, which a clock behind by more than a second makes of every such proof under the default window (rule 5); in review in Hagency Desktop (#246). Not yet for the activity lease: its store still refuses an activity lease that reads longer than allowed by the wall clock, which a slow clock makes of every fresh lease; in review in Hagency Desktop (#243, #251). Not yet for the Agent execution lease, which Hagency Desktop reads without the answer's date (rule 5); in review in Hagency Desktop (#246) |

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
  window (in review, #97) and are renewed when they have lost 10 s (in review
  in Hagency Desktop), the lead becomes the window less about 10 s: about
  110 s under a 120 s window.
  What such a process had been given and not begun is rule 3's subject.
- **An Agent with an applied budget has no further attempts.** Its protocol
  has no retry: a start that fails ends the message. Its status says so and
  its sender can send it again, as for every other message that ended
  without a turn. A message that the earlier protocol had started when the
  budget was applied ends the same way (`head_changed`), and one that it had
  already ended keeps that end: the budget's queue can take neither up, and
  offers the next message instead of waiting behind it. A message that the
  earlier protocol had given to a process, which had not begun it, does not
  begin either, whether it came by direct routing or through a v2 binding:
  that process's next work poll ends it without a turn (`cancelled`, which
  the status reads as `never_started`, as for work given to a process that
  never began it), and an acknowledgement or a start of it is refused
  (`shared_work_definitively_not_started`). Only the budget's process is
  held to the budget's head and ceiling. Work that had begun is left alone:
  its end is unknown, and it is never run again.
- **A block on usage is not settled.** A message that the computer blocked
  on usage ends at once, with or without an applied budget (rule 3): the
  computer cannot account for the Agent's earlier calls and keeps that until
  the period turns, for an Agent without periods for good. Only a way for
  the computer to settle such calls would let a later message run, and that
  is a change of its accounting, not of a time limit.
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

   The server must also be one that grants a longer window only through the
   second header (rule 6). An earlier server grants it through the first
   header too, and so to Hagency Desktop 0.5.0-rc.19, which makes one local
   session per call and can then refuse its own calls (failure 2).
2. **Then the computers.** A window above 30 s is safe only for computers
   whose daemon shares one local session among its calls, and such a daemon
   says so with the second header. Every other computer keeps working at
   30 s: the server does not grant it a longer window, whatever is
   configured. That includes a build that shares its local session but sends
   only the first header; it runs at 30 s until it sends the second.
3. **Then raise the window.** It takes effect as each session is next
   renewed, for the computers that send the second header. Such a computer
   reads a proof with a longer window only by the answer's `Date` header
   (rule 5), so a proxy in front of the server that drops or rewrites that
   header, or adds an `Age`, makes it refuse its own proof: check the proxy
   first.

Going back:

- Lowering the window again is safe once step 1 is done.
- Moving the server back to an older binary is safe for what it reads: the
  older binary reads everything a newer one wrote, because an offer never
  exceeds 30 seconds. A binary from before the second header (rule 6),
  though, grants a longer window through the first header, so to Hagency
  Desktop 0.5.0-rc.19 too. While a computer may run that release, lower the
  window to 30 s before moving the server back so far.
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
  the window (in review, #97), has a lower number than `000155` and `000156`. A
  database that already has those applies it out of version order, because
  the server applies every migration it has not recorded, whatever its
  number. That is harmless: it only replaces the CHECK constraint on the
  epochs of activity renewal rows, which neither later migration reads or
  changes.
- This release acts on `reoffers` (rule 3): "Bringing the re-offer into
  service, and back" below has its order and its way back.

## Bringing the re-offer into service, and back

Rule 3's re-offer has its own switch, `enable_shared_demand_reoffers` in the
host's management configuration. It is off by default, and it takes effect
only together with demand retries (`shared-demand-retry-v1`), which keep
their own switches and keep working when it is off. While it is on,
discovery lists `shared-demand-retry-v2`, and a computer says `reoffers`
only while discovery lists it. The service also sets the database session
setting `hagency.shared_demand_reoffers` in every transaction; the database
takes a message back only when that setting is `on`, so a binary that does
not set it (an older release) takes nothing back.

In order:

1. **A server that reads `reoffers` first** (rule 6): a release from before
   it refuses every demand request that carries the field.
2. **Then this release, with the switch off.** It behaves as the reader,
   except that it opens a retry only where the retry's start would be
   accepted, and the fence records say more (the operations guide).
3. **Then switch it on.** Computers that support it start saying `reoffers`
   as they next read discovery.

Going back:

1. **Switch it off** (and restart the service). Nothing more is taken back:
   a stop on the retry route is refused as for a start that did not say
   `reoffers`, and the computer reports it on the plain route. A message
   that was already taken back still ends visibly: at its bound
   (`retry_lapsed`), or earlier when its computer reads it without
   `reoffers` (`reoffer_unsupported`), or at the computer's next demand
   poll when the stop that gave it back came on the plain route and no
   retry was asked for (`head_changed`). A retry that was already opened
   may still be started; if that process also ends before its turn, the
   message ends as for any start, as before.
2. **Let those answers settle**: until no message that was taken back is
   still waiting. Each one ends by its bound (the table above), so this
   takes minutes; the operations guide has the query.
3. **Then move the binary back**, to the reader release (rule 6) or a later
   one. Going further back needs every start that said `reoffers` finished
   first: a computer sends a saved start again with its original bytes.
4. **The down migration of `2026-11-27-000159` later, if at all.** It
   removes the trigger that takes messages back, puts the four guards it
   relaxed and the guard on work starts back as they were, and puts back
   the fence record of `2026-11-26-000156` without the release counts and
   the stop source. It keeps the record of what was taken back
   (`management_shared_demand_released_bindings`), that table's guards, and
   the functions `shared_work_never_began()`, `shared_start_reoffers()`,
   `shared_agent_serves()` and `shared_agent_budget_applied()`, which that
   table's guards use (and this release's chat status). Run it only after
   the binary went back: this release applies the migration again when it
   next starts. Without the down migration, an older binary takes nothing
   back, because it does not set the session setting.
