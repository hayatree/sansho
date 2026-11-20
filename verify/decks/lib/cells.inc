* Resistor-loaded NMOS inverter.
* Provenance: original Sansho verification fixture.
.subckt rinv in out vdd params: l=0.5u w=2u rl=20k
RL vdd out {rl}
M1 out in 0 0 nch l={l} w={w}
.ends
