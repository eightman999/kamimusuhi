"""M1A — recurrent substrate validation & repair.

Diagnostic harness for the M1A gate: prove that genome-derived neural
parameters causally affect state transitions and spiking inside the
recurrent substrate, on a small deterministic network, at meso scale,
and at production scale (139k neurons / 14M edges).

Nothing here runs evolution. Everything is a controlled comparison:
same input, same seeds, one manipulated variable.
"""
