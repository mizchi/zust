---------------------------- MODULE HashMapModel ----------------------------
(* What a HashMap does, as a model the implementation is tested against (formal/connect: zustc's prelude HashMap,
   compiled by zustc, follows it). m maps each key to its value, 0 for none.

   For formal/connect, `act` is the action a step takes and `obs` what it observes; they are not part of the state
   the implementation is compared on (the connect crate projects them out). Init sets act to <<"reset">>. *)
EXTENDS Naturals, FiniteSets

CONSTANTS Keys, Vals

None == 0

VARIABLES m, act, obs

vars == <<m, act, obs>>

Init ==
    /\ m = [k \in Keys |-> None]
    /\ act = <<"reset">>
    /\ obs = <<"ok">>

Insert(k, v) ==
    /\ m' = [m EXCEPT ![k] = v]
    /\ act' = <<"insert", k, v>>
    /\ obs' = <<"prev", m[k]>>

Remove(k) ==
    /\ m' = [m EXCEPT ![k] = None]
    /\ act' = <<"remove", k>>
    /\ obs' = <<"prev", m[k]>>

Get(k) ==
    /\ m' = m
    /\ act' = <<"get", k>>
    /\ obs' = <<"val", m[k]>>

Len ==
    /\ m' = m
    /\ act' = <<"len">>
    /\ obs' = <<"len", Cardinality({k \in Keys : m[k] # None})>>

Next ==
    \/ \E k \in Keys, v \in Vals : Insert(k, v)
    \/ \E k \in Keys : Remove(k) \/ Get(k)
    \/ Len

TypeOK == \A k \in Keys : m[k] \in Vals \cup {None}
=============================================================================
