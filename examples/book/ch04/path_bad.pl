path(X,Z) :- path(X,Y), edge(Y,Z).
path(X,Y) :- edge(X,Y).
edge(a,b). edge(b,c). edge(c,d).
