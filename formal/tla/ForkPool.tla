----------------------------- MODULE ForkPool -----------------------------
(* The fork/pipe worker pool of zustc's driver (crates/core/src/session.rs, item summaries;
   crates/core/src/session/build.rs, functions and crates).

   The parent makes pipe k and forks child k, which computes items j with j % Jobs = k - 1 and writes one record
   per item to the pipe. The parent then reads pipe 1 to EOF, waits for child 1, reads pipe 2, ... and finally
   computes itself every item no child returned.

   What can go wrong, modelled:
   - fork fails (the parent skips the child);
   - a child dies at any point, or its write stops early (a signal, a short write: `write_all` gives up on
     n <= 0), leaving a record cut in the middle; the reader stops at a cut record (`r.take(n)` fails);
   - a pipe holds Cap records; a writer blocks while it is full;
   - a child inherits every pipe write end the parent holds at the time of the fork. EOF on pipe k needs every
     holder of its write end to close it.

   CloseLate = FALSE is the code: the parent closes the write end of pipe k right after forking child k.
   CloseLate = TRUE closes them only after forking every child: then child 2 holds pipe 1's write end, and the
   model checker finds the deadlock (the parent waits for EOF on pipe 1 while child 2 waits for room in pipe 2). *)
EXTENDS Naturals, Sequences

CONSTANTS Jobs, N, Cap, CloseLate

Kids == 1..Jobs
Procs == 0..Jobs
Items == 0..(N - 1)
Count(k) == (N - (k - 1) + Jobs - 1) \div Jobs
ItemOf(k, i) == (k - 1) + (i - 1) * Jobs
None == 0 - 1

VARIABLES pstage, cur, wr, kid, sent, pipe, broken, res

vars == <<pstage, cur, wr, kid, sent, pipe, broken, res>>

Init ==
    /\ pstage = "fork"
    /\ cur = 1
    /\ wr = [p \in Kids |-> {}]
    /\ kid = [k \in Kids |-> "unborn"]
    /\ sent = [k \in Kids |-> 0]
    /\ pipe = [k \in Kids |-> <<>>]
    /\ broken = [k \in Kids |-> FALSE]
    /\ res = [j \in Items |-> None]

(* pipe(); fork() succeeds. The child holds what the parent held, and its own write end *)
ForkOk ==
    /\ pstage = "fork" /\ cur <= Jobs
    /\ wr' = [p \in Kids |->
                IF p = cur THEN (IF CloseLate THEN {0, cur} ELSE {cur})
                ELSE IF 0 \in wr[p] THEN wr[p] \cup {cur} ELSE wr[p]]
    /\ kid' = [kid EXCEPT ![cur] = "run"]
    /\ cur' = cur + 1
    /\ UNCHANGED <<pstage, sent, pipe, broken, res>>

ForkFail ==
    /\ pstage = "fork" /\ cur <= Jobs
    /\ kid' = [kid EXCEPT ![cur] = "failed"]
    /\ cur' = cur + 1
    /\ UNCHANGED <<pstage, wr, sent, pipe, broken, res>>

StartRead ==
    /\ pstage = "fork" /\ cur > Jobs
    /\ wr' = [p \in Kids |-> wr[p] \ {0}]
    /\ pstage' = "read" /\ cur' = 1
    /\ UNCHANGED <<kid, sent, pipe, broken, res>>

Exit(k) == wr' = [p \in Kids |-> wr[p] \ {k}]

Send(k) ==
    /\ kid[k] = "run" /\ sent[k] < Count(k) /\ Len(pipe[k]) < Cap
    /\ pipe' = [pipe EXCEPT ![k] = Append(pipe[k], <<ItemOf(k, sent[k] + 1), TRUE>>)]
    /\ sent' = [sent EXCEPT ![k] = sent[k] + 1]
    /\ UNCHANGED <<pstage, cur, wr, kid, broken, res>>

Finish(k) ==
    /\ kid[k] = "run" /\ sent[k] = Count(k)
    /\ kid' = [kid EXCEPT ![k] = "exit"]
    /\ Exit(k)
    /\ UNCHANGED <<pstage, cur, sent, pipe, broken, res>>

(* dies or stops writing; maybe in the middle of a record. A child blocked in write() (its pipe full) stays
   blocked: nothing kills it *)
Die(k) ==
    /\ kid[k] = "run" /\ (Len(pipe[k]) < Cap \/ sent[k] = Count(k))
    /\ \/ UNCHANGED pipe
       \/ /\ sent[k] < Count(k) /\ Len(pipe[k]) < Cap
          /\ pipe' = [pipe EXCEPT ![k] = Append(pipe[k], <<ItemOf(k, sent[k] + 1), FALSE>>)]
    /\ kid' = [kid EXCEPT ![k] = "exit"]
    /\ Exit(k)
    /\ UNCHANGED <<pstage, cur, sent, broken, res>>

(* the parent decodes the next record of pipe cur *)
Recv ==
    /\ pstage = "read" /\ cur <= Jobs /\ Len(pipe[cur]) > 0
    /\ LET r == Head(pipe[cur]) IN
       /\ IF ~broken[cur] /\ r[2] THEN res' = [res EXCEPT ![r[1]] = r[1]] ELSE UNCHANGED res
       /\ broken' = [broken EXCEPT ![cur] = broken[cur] \/ ~r[2]]
    /\ pipe' = [pipe EXCEPT ![cur] = Tail(pipe[cur])]
    /\ UNCHANGED <<pstage, cur, wr, kid, sent>>

(* read() returns 0 (EOF), then waitpid *)
Eof ==
    /\ pstage = "read" /\ cur <= Jobs
    /\ \/ kid[cur] = "failed"
       \/ Len(pipe[cur]) = 0 /\ wr[cur] = {} /\ kid[cur] = "exit"
    /\ cur' = cur + 1
    /\ UNCHANGED <<pstage, wr, kid, sent, pipe, broken, res>>

(* the parent computes what no child returned *)
Fallback ==
    /\ pstage = "read" /\ cur > Jobs
    /\ res' = [j \in Items |-> j]
    /\ pstage' = "done"
    /\ UNCHANGED <<cur, wr, kid, sent, pipe, broken>>

Done == pstage = "done" /\ UNCHANGED vars

Next == ForkOk \/ ForkFail \/ StartRead \/ Recv \/ Eof \/ Fallback \/ Done
        \/ \E k \in Kids : Send(k) \/ Finish(k) \/ Die(k)

(* a value the parent takes from a child is the right one *)
NoWrongValue == \A j \in Items : res[j] \in {None, j}
Correct == pstage = "done" => \A j \in Items : res[j] = j
=============================================================================
