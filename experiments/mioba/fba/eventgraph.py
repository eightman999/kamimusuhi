"""Event-driven synaptic propagation (M1 §2).

Profiling the M0 evaluator (``mioba profile``) showed ~86% of every
simulation step inside one call:

    torch.sparse.mm(W, spikes.T)        # W: N x N sparse, spikes: B x N

That product multiplies the whole connectome by a vector that is almost
entirely zero. At FlyWire scale with M0's 9.5 Hz population rate and
dt = 0.1 ms, a neuron spikes in a given step with probability
9.5 x 1e-4 = 0.095% (about 1 in 1,050), so ~132 of 139,000 columns
contribute. At a mean out-degree of 14,000,000 / 139,000 = ~100 edges,
that is ~13,000 of 14,000,000 edges carrying anything. The other 99.9%
of the multiply-adds are multiplications by zero.

``EventGraph`` stores the same weights in **presynaptic-major (CSC)**
order, so the edges leaving a spiking neuron are one contiguous slice.
A step gathers only the slices of the neurons that actually spiked and
scatter-adds them into the postsynaptic current:

    I[b, post] = sum over spiking pre of  W[post, pre] * spikes[b, pre]

which is the same sum as the matmul, evaluated only where the factor is
non-zero. Floating-point summation order differs from a dense product
(index_add_ accumulates in gather order), the same class of difference
as CUDA sparse atomics.

The cost is O(spikes + edges leaving them) instead of O(nnz), so it
*scales with activity*: a quiet organism is cheap and a pathologically
hyperactive one costs at most what the dense product always cost.
"""
from __future__ import annotations

try:
    import torch
except ImportError:  # pragma: no cover
    torch = None


class EventGraph:
    """Weights in presynaptic-major (CSC) layout.

    ``colptr[p]:colptr[p+1]`` is the slice of ``row`` / ``val`` holding the
    outgoing edges of presynaptic neuron ``p``. Built by coalescing a COO
    edge list, so duplicate edges are summed exactly as they were when the
    graph was one CSR matrix.
    """

    __slots__ = ("colptr", "row", "val", "n_rows", "n_cols", "nnz")

    def __init__(self, colptr, row, val, n_rows: int, n_cols: int):
        self.colptr = colptr
        self.row = row
        self.val = val
        self.n_rows = int(n_rows)
        self.n_cols = int(n_cols)
        self.nnz = int(val.numel())

    # ------------------------------------------------------------- build
    @classmethod
    def from_coo(cls, post, pre, val, n_rows: int, n_cols: int) -> "EventGraph":
        """``post``/``pre``/``val`` are parallel 1-D tensors of directed
        edges pre -> post. Transposing the COO before the CSR conversion
        gives presynaptic-major order directly."""
        t = torch.sparse_coo_tensor(torch.stack([pre, post]), val,
                                    (n_cols, n_rows)).coalesce()
        csr = t.to_sparse_csr()
        return cls(csr.crow_indices(), csr.col_indices(), csr.values(),
                   n_rows, n_cols)

    def to(self, device) -> "EventGraph":
        return EventGraph(self.colptr.to(device), self.row.to(device),
                          self.val.to(device), self.n_rows, self.n_cols)

    def scaled(self, factor: float) -> "EventGraph":
        return EventGraph(self.colptr, self.row, self.val * factor,
                          self.n_rows, self.n_cols)

    # -------------------------------------------------------- inspection
    def to_coo(self, size: tuple[int, int] | None = None, scale: float = 1.0):
        """Materialise ``W[post, pre]`` as a COO tensor (tests, replay
        diffing, GUI circuit views). The simulation never calls this."""
        counts = self.colptr[1:] - self.colptr[:-1]
        pre = torch.repeat_interleave(
            torch.arange(self.n_cols, device=self.colptr.device), counts)
        shape = size or (self.n_rows, self.n_cols)
        return torch.sparse_coo_tensor(torch.stack([self.row, pre]),
                                       self.val * scale, shape)

    # ------------------------------------------------------------- step
    def propagate(self, spikes, out, scale: float = 1.0,
                  col_limit: int | None = None) -> None:
        """Accumulate this graph's contribution of ``spikes`` (B x N_any,
        non-zero entries are the events) into ``out`` (B x N_out).

        ``col_limit`` restricts the presynaptic side to the first
        ``col_limit`` columns — used for the FBA0 base graph, whose
        columns cover only the base neurons while ``spikes`` also carries
        the artificial ones.
        """
        idx = torch.nonzero(spikes, as_tuple=False)
        if idx.numel() == 0:
            return
        lane, pre = idx[:, 0], idx[:, 1]
        if col_limit is not None:
            keep = pre < col_limit
            if not bool(keep.all()):
                lane, pre = lane[keep], pre[keep]
                if pre.numel() == 0:
                    return
        amp = spikes[lane, pre]
        start = self.colptr[pre]
        counts = self.colptr[pre + 1] - start
        total = int(counts.sum())
        if total == 0:
            return
        # flat positions of every outgoing edge of every spiking neuron
        seg_start = torch.cumsum(counts, 0) - counts
        pos = (torch.repeat_interleave(start, counts)
               + torch.arange(total, device=start.device)
               - torch.repeat_interleave(seg_start, counts))
        rows = self.row[pos]
        vals = self.val[pos] * torch.repeat_interleave(amp, counts)
        if scale != 1.0:
            vals = vals * scale
        n_out = out.shape[1]
        flat = torch.repeat_interleave(lane, counts) * n_out + rows
        out.view(-1).index_add_(0, flat, vals)
