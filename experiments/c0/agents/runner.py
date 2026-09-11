"""Runner: binds a policy to its mutable per-episode state.

The runner owns everything that can cross a process boundary:

    hidden   recurrent state (N, H) -- None for stateless policies
    memory   per-env SlotMemory     -- only for mem* models

reset modes ("all", "hidden", "memory", "none") let training simulate every
restore condition: a mid-episode reset of the hidden state alone leaves the
memory intact (C-B2-like), a memory clear keeps hidden (C-B1-like), etc.
"""
from __future__ import annotations

import numpy as np
import torch

from ..env.c0_env import C0Config, ITEM, NOISE, STORE
from .policies import MODEL_SPECS
from .slot_memory import SlotMemory


class Runner:
    def __init__(self, policy: torch.nn.Module, model_name: str,
                 env_cfg: C0Config, device="cpu"):
        self.policy = policy
        self.model_name = model_name
        self.spec = MODEL_SPECS[model_name]
        self.cfg = env_cfg
        self.device = torch.device(device)
        self.has_memory = self.spec["memory"]
        self.n = 0
        self.h = None
        self.mems: list[SlotMemory] = []

    # ------------------------------------------------------------------
    def reset(self, n: int) -> None:
        self.n = n
        self.h = self.policy.initial_state(n, self.device)
        self.mems = [SlotMemory(self.cfg.memory_slots, self.cfg.payload_dim,
                                self.cfg.num_keys)
                     for _ in range(n)] if self.has_memory else []

    def reset_where(self, idx: np.ndarray, mode: str) -> None:
        """Partial mid-episode reset used by training restart augmentation."""
        if mode == "none" or len(idx) == 0:
            return
        if mode in ("all", "hidden") and self.h is not None:
            self.h[idx] = 0.0
        if mode in ("all", "memory"):
            for i in idx:
                self.mems[int(i)].clear()

    # ------------------------------------------------------------------
    def _input(self, obs: np.ndarray) -> torch.Tensor:
        if self.has_memory:
            mem = np.stack([m.contents().reshape(-1) for m in self.mems])
            obs = np.concatenate([obs, mem], axis=1)
        return torch.as_tensor(obs, dtype=torch.float32, device=self.device)

    def act(self, obs: np.ndarray, greedy: bool = False,
            rng: np.random.Generator | None = None):
        """One batched step. Applies STORE writes to the agent memory."""
        x = self._input(obs)
        with torch.no_grad():
            al, nl, _v, self.h = self.policy(x, self.h)
        if greedy:
            acts = al.argmax(-1).cpu().numpy()
            ans = nl.argmax(-1).cpu().numpy()
        else:
            acts = torch.distributions.Categorical(logits=al).sample().cpu().numpy()
            ans = torch.distributions.Categorical(logits=nl).sample().cpu().numpy()
        # STORE side effect: write the current event payload into memory
        if self.has_memory:
            c = self.cfg
            for i, a in enumerate(acts):
                if a != STORE:
                    continue
                ev = obs[i, :c.payload_dim]
                kind = int(np.argmax(ev[:4]))
                if kind not in (ITEM, NOISE):
                    continue
                key = int(np.argmax(ev[4:4 + c.num_keys])) if kind == ITEM else -1
                self.mems[i].store(ev, key)
        return acts, ans

    def hidden(self) -> np.ndarray | None:
        return None if self.h is None else self.h.cpu().numpy()

    # -- state in/out ----------------------------------------------------
    def state_fields(self) -> list[str]:
        fields = [] if self.h is None else ["hidden"]
        if self.has_memory:
            fields += ["mem_payloads", "mem_keys", "mem_occupied",
                       "mem_insert", "mem_clock"]
        return fields

    def get_states(self) -> list[dict]:
        """Per-env state dicts (arrays are unbatched copies)."""
        out = []
        hid = self.hidden()
        for i in range(self.n):
            st = {}
            if hid is not None:
                st["hidden"] = hid[i].astype(np.float32)
            if self.has_memory:
                st.update(self.mems[i].clone_state())
            out.append(st)
        return out

    def set_states(self, states: list[dict]) -> None:
        """Restore per-env states; absent fields mean fresh (cold)."""
        assert len(states) == self.n
        if self.h is not None:
            h = np.zeros_like(self.h.cpu().numpy())
            for i, st in enumerate(states):
                if "hidden" in st:
                    h[i] = st["hidden"]
            self.h = torch.as_tensor(h, device=self.device)
        for i, st in enumerate(states):
            if not self.has_memory:
                break
            self.mems[i].clear()
            if "mem_payloads" in st:
                self.mems[i].load_state(st)
