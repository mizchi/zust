---------------------------- MODULE CacheSave ----------------------------
(* Two zustc processes save the build cache at the same time.
   Code: crates/typeck/src/cache2.rs `Cache2::save` (write a tmp file, then rename it over `<path>`; src/stamp.rs
   saves the stamps the same way) and
   `Cache2::open` (accepts a file whose header and lengths are in bounds; there is no checksum).

   A file is a sequence of cells. Writer w writes the cells <<w, 1>> .. <<w, Size[w]>>; cell 1 is the header, which
   says how long the file is. One write step writes one cell, at the writer's own offset (POSIX: a read may see a
   write that is only partly done; O_TRUNC of a file another writer is still writing leaves a hole <<0, 0>>).
   A reader that finds the header and that many cells accepts the file: Integrity says that what it accepts is one
   writer's file.

   SharedTmp = FALSE is the code (CacheSave.cfg): a tmp name per process, `<path>.<pid>.tmp`.
   SharedTmp = TRUE is the code before 2026-11-20 (CacheSaveShared.cfg): one tmp name for every process. Then B can
   reopen A's tmp with O_TRUNC, A renames it, B goes on writing into what is now <path>, and a reader accepts B's
   header with the rest of A's file: Integrity fails. *)
EXTENDS Naturals, Sequences

CONSTANTS SharedTmp

Writers == {1, 2}
Size == [w \in Writers |-> IF w = 1 THEN 3 ELSE 2]
Inodes == 1..3
NoIno == 0
Hole == <<0, 0>>

VARIABLES pc, fd, off, names, data, nextIno, seen

vars == <<pc, fd, off, names, data, nextIno, seen>>

TmpName(w) == IF SharedTmp THEN "tmp" ELSE IF w = 1 THEN "tmp1" ELSE "tmp2"
Names == {"path", "tmp", "tmp1", "tmp2"}

Init ==
    /\ pc = [w \in Writers |-> "open"]
    /\ fd = [w \in Writers |-> NoIno]
    /\ off = [w \in Writers |-> 1]
    /\ names = [n \in Names |-> NoIno]
    /\ data = [i \in Inodes |-> <<>>]
    /\ nextIno = 1
    /\ seen = "none"

(* std::fs::write: open(O_CREAT | O_TRUNC) *)
Open(w) ==
    /\ pc[w] = "open"
    /\ IF names[TmpName(w)] # NoIno
       THEN /\ fd' = [fd EXCEPT ![w] = names[TmpName(w)]]
            /\ data' = [data EXCEPT ![names[TmpName(w)]] = <<>>]
            /\ UNCHANGED <<names, nextIno>>
       ELSE /\ nextIno \in Inodes
            /\ fd' = [fd EXCEPT ![w] = nextIno]
            /\ names' = [names EXCEPT ![TmpName(w)] = nextIno]
            /\ nextIno' = nextIno + 1
            /\ UNCHANGED data
    /\ off' = [off EXCEPT ![w] = 1]
    /\ pc' = [pc EXCEPT ![w] = "write"]
    /\ UNCHANGED seen

WriteAt(c, p, x) == [i \in 1..(IF Len(c) >= p THEN Len(c) ELSE p) |-> IF i = p THEN x ELSE IF i <= Len(c) THEN c[i] ELSE Hole]

Write(w) ==
    /\ pc[w] = "write"
    /\ data' = [data EXCEPT ![fd[w]] = WriteAt(data[fd[w]], off[w], <<w, off[w]>>)]
    /\ off' = [off EXCEPT ![w] = off[w] + 1]
    /\ pc' = [pc EXCEPT ![w] = IF off[w] = Size[w] THEN "rename" ELSE "write"]
    /\ UNCHANGED <<fd, names, nextIno, seen>>

(* rename(tmp, path): fails (the save returns an error) if tmp is gone *)
Rename(w) ==
    /\ pc[w] = "rename"
    /\ IF names[TmpName(w)] # NoIno
       THEN names' = [names EXCEPT !["path"] = names[TmpName(w)], ![TmpName(w)] = NoIno]
       ELSE UNCHANGED names
    /\ pc' = [pc EXCEPT ![w] = "done"]
    /\ UNCHANGED <<fd, off, data, nextIno, seen>>

(* Cache2::open on `path` (the file as it is at this moment) *)
Read ==
    /\ names["path"] # NoIno
    /\ LET c == data[names["path"]] IN
       /\ Len(c) >= 1
       /\ c[1][1] # 0
       /\ LET hw == c[1][1] IN
          IF Len(c) >= Size[hw]
          THEN seen' = IF \A i \in 1..Size[hw] : c[i] = <<hw, i>> THEN "whole" ELSE "mixed"
          ELSE seen' = "rejected"
    /\ UNCHANGED <<pc, fd, off, names, data, nextIno>>

Done == /\ \A w \in Writers : pc[w] = "done"
        /\ UNCHANGED vars

Next == (\E w \in Writers : Open(w) \/ Write(w) \/ Rename(w)) \/ Read \/ Done

Integrity == seen # "mixed"

(* After both saves finish, path is one writer's complete file *)
FinalWhole == (\A w \in Writers : pc[w] = "done") =>
    (names["path"] # NoIno /\ \E w \in Writers : data[names["path"]] = [i \in 1..Size[w] |-> <<w, i>>])

Spec == Init /\ [][Next]_vars
=============================================================================
