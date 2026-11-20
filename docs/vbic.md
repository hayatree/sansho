# Native VBIC coverage

The native Q device supports the level-one Gummel–Poon transistor. A level-four NPN or PNP card cannot currently run as a native VBIC device. For VBIC, load a user-supplied Verilog-A model with `.hdl` and use its declared terminal order. The SG13G2 Gummel sweep demonstrates the five-terminal VBIC 1.3 route. [Example circuit](../verify/va-decks/vbic_gummel.cir)

The requested physical definition is C. C. McAndrew et al., “VBIC95, the vertical bipolar inter-company model,” *IEEE Journal of Solid-State Circuits* 31(10), 1996, together with the public VBIC model definition. The ngspice manual specifies the netlist parameter names and external terminal order. These references were unavailable in this offline round. There are no native level-four Gummel, output-curve, fT or self-heating results to report.
