:- use_module(library(clpfd)).
queens(N, Qs) :-
    length(Qs, N), Qs ins 1..N,
    safe(Qs), all_different(Qs).
safe([]).
safe([Q|Qs]) :- no_attack(Q, Qs, 1), safe(Qs).
no_attack(_, [], _).
no_attack(Q, [Q1|Qs], D) :-
    Q #\= Q1 + D, Q #\= Q1 - D, D1 #= D + 1, no_attack(Q, Qs, D1).
