* Level-1 NMOS whose threshold depends on the corner parameter dvt.
.model nch nmos level=1 vto={0.5+dvt} kp=120u lambda={0.08*(1+dvt)}
