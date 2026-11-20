------------------------------ MODULE ArcDrop ------------------------------
(* zustc's `Arc` is `Rc` (crates/syntax/src/prelude_lib.rs.in: `strong += 1`, `strong -= 1; if strong == 0 { free }`)
   and `std::thread::spawn` runs the closure at once, so there is one thread and this is sound.

   This spec is the check to pass before zustc gets real threads: Threads threads each drop one clone of an Arc.
   Atomic = FALSE is the prelude's code as it is (the decrement is a load and a store); the model checker finds a
   lost update (the box is never freed: a leak) and, with a clone racing a drop, a free while a clone is still held
   (use after free). Atomic = TRUE is `fetch_sub(1) == 1` (std's algorithm, under sequential consistency: the
   Release/Acquire fence argument of RbRlx is outside this model). *)
EXTENDS Naturals

CONSTANTS Threads, Atomic

T == 1..Threads

VARIABLES strong, pc, tmp, freed, frees, holds

vars == <<strong, pc, tmp, freed, frees, holds>>

(* thread 1 clones its Arc before dropping it (one more clone in flight); every thread then drops what it holds *)
Init ==
    /\ strong = Threads
    /\ pc = [t \in T |-> IF t = 1 THEN "clone" ELSE "drop"]
    /\ tmp = [t \in T |-> 0]
    /\ freed = FALSE
    /\ frees = 0
    /\ holds = [t \in T |-> 1]

(* clone: strong += 1 (as a load then a store unless Atomic) *)
CloneLoad(t) == pc[t] = "clone" /\ ~Atomic /\ tmp' = [tmp EXCEPT ![t] = strong] /\ pc' = [pc EXCEPT ![t] = "cloneStore"]
                /\ UNCHANGED <<strong, freed, frees, holds>>
CloneStore(t) == pc[t] = "cloneStore" /\ strong' = tmp[t] + 1 /\ holds' = [holds EXCEPT ![t] = holds[t] + 1]
                 /\ pc' = [pc EXCEPT ![t] = "drop"] /\ UNCHANGED <<tmp, freed, frees>>
CloneAtomic(t) == pc[t] = "clone" /\ Atomic /\ strong' = strong + 1 /\ holds' = [holds EXCEPT ![t] = holds[t] + 1]
                  /\ pc' = [pc EXCEPT ![t] = "drop"] /\ UNCHANGED <<tmp, freed, frees>>

(* drop: strong -= 1; if strong == 0 { free } *)
DropLoad(t) == pc[t] = "drop" /\ holds[t] > 0 /\ ~Atomic /\ tmp' = [tmp EXCEPT ![t] = strong] /\ pc' = [pc EXCEPT ![t] = "dropStore"]
               /\ UNCHANGED <<strong, freed, frees, holds>>
DropStore(t) == pc[t] = "dropStore" /\ strong' = tmp[t] - 1 /\ holds' = [holds EXCEPT ![t] = holds[t] - 1]
                /\ pc' = [pc EXCEPT ![t] = "test"] /\ UNCHANGED <<tmp, freed, frees>>
Test(t) == pc[t] = "test"
           /\ (IF strong = 0 THEN freed' = TRUE /\ frees' = frees + 1 ELSE UNCHANGED <<freed, frees>>)
           /\ pc' = [pc EXCEPT ![t] = "drop"] /\ UNCHANGED <<strong, tmp, holds>>
(* fetch_sub(1) == 1 *)
DropAtomic(t) == pc[t] = "drop" /\ holds[t] > 0 /\ Atomic
                 /\ strong' = strong - 1 /\ holds' = [holds EXCEPT ![t] = holds[t] - 1]
                 /\ (IF strong = 1 THEN freed' = TRUE /\ frees' = frees + 1 ELSE UNCHANGED <<freed, frees>>)
                 /\ UNCHANGED <<pc, tmp>>

Finished == \A t \in T : pc[t] = "drop" /\ holds[t] = 0
Done == Finished /\ UNCHANGED vars

Next == Done \/ \E t \in T : CloneLoad(t) \/ CloneStore(t) \/ CloneAtomic(t) \/ DropLoad(t) \/ DropStore(t) \/ Test(t) \/ DropAtomic(t)

(* never freed while someone holds a clone, never freed twice *)
NoUseAfterFree == freed => \A t \in T : holds[t] = 0
NoDoubleFree == frees <= 1
(* every clone dropped: freed exactly once *)
NoLeak == Finished => frees = 1
=============================================================================
