"""Linear probes (numpy/torch, no sklearn).

All probe labels (cause ids, boundary flags, best-action classes) are
env ground truth used at EVALUATION time only.
"""

from __future__ import annotations

from typing import Optional, Tuple

import numpy as np
import torch


def logistic_probe(Xtr: np.ndarray, ytr: np.ndarray, Xte: np.ndarray,
                   yte: Optional[np.ndarray] = None,
                   n_classes: Optional[int] = None, steps: int = 800,
                   lr: float = 0.05, seed: int = 0,
                   weight: Optional[np.ndarray] = None) -> dict:
    """Multinomial logistic regression probe. Returns acc + predictions
    + confusion matrix (rows = true class, cols = predicted)."""
    Xtr = np.asarray(Xtr, dtype=np.float32)
    Xte = np.asarray(Xte, dtype=np.float32)
    ytr = np.asarray(ytr, dtype=np.int64)
    if n_classes is None:
        n_classes = int(max(ytr.max(), yte.max() if yte is not None else 0)) + 1
    # standardize features for conditioning
    mu, sd = Xtr.mean(0), Xtr.std(0) + 1e-8
    Xtr_s, Xte_s = (Xtr - mu) / sd, (Xte - mu) / sd

    torch.manual_seed(seed)
    clf = torch.nn.Linear(Xtr_s.shape[1], n_classes)
    opt = torch.optim.Adam(clf.parameters(), lr=lr)
    xt = torch.as_tensor(Xtr_s)
    yt = torch.as_tensor(ytr)
    if weight is not None:
        w = torch.as_tensor(np.asarray(weight, dtype=np.float32))
        loss_fn = torch.nn.CrossEntropyLoss(weight=w / w.mean())
    else:
        loss_fn = torch.nn.CrossEntropyLoss()
    for _ in range(steps):
        opt.zero_grad()
        loss = loss_fn(clf(xt), yt)
        loss.backward()
        opt.step()
    with torch.no_grad():
        logits = clf(torch.as_tensor(Xte_s))
        prob = torch.softmax(logits, dim=1).numpy()
        pred = logits.argmax(1).numpy()
    out = {"pred": pred, "prob": prob, "prob_pos": prob[:, -1]}
    if yte is not None:
        yte = np.asarray(yte, dtype=np.int64)
        out["acc"] = float((pred == yte).mean())
        conf = np.zeros((n_classes, n_classes))
        for t, p in zip(yte, pred):
            if 0 <= t < n_classes:
                conf[t, p] += 1
        rowsum = conf.sum(1, keepdims=True)
        out["confusion"] = (conf / np.maximum(rowsum, 1)).tolist()
        out["acc_per_class"] = [
            float(conf[i, i] / rowsum[i, 0]) if rowsum[i, 0] > 0 else None
            for i in range(n_classes)]
        if n_classes == 2:
            from ..analysis.metrics import auc_score
            out["auc"] = auc_score(yte == 1, prob[:, 1])
    return out


def ridge_probe(Xtr: np.ndarray, Ytr: np.ndarray, Xte: np.ndarray,
                Yte: np.ndarray, lam: float = 1e-3) -> dict:
    """Linear regression probe; returns per-target R2."""
    Xb = np.concatenate([Xtr, np.ones((len(Xtr), 1))], axis=1)
    A = Xb.T @ Xb + lam * np.eye(Xb.shape[1])
    W = np.linalg.solve(A, Xb.T @ Ytr)
    pred = np.concatenate([Xte, np.ones((len(Xte), 1))], axis=1) @ W
    ss_res = ((Yte - pred) ** 2).sum(0)
    ss_tot = ((Yte - Yte.mean(0)) ** 2).sum(0) + 1e-12
    return {"r2": (1.0 - ss_res / ss_tot).tolist()}


def fewshot_probe(X: np.ndarray, y: np.ndarray, shots: int,
                  n_classes: int, seed: int = 0, steps: int = 800,
                  lr: float = 0.05, repeats: int = 5) -> dict:
    """m-shot classification: sample `shots` examples per class for
    training, evaluate on the rest. Repeated `repeats` times."""
    rng = np.random.default_rng(seed)
    accs = []
    for rep in range(repeats):
        tr_idx, te_idx = [], []
        for c in range(n_classes):
            idx = np.where(y == c)[0]
            if len(idx) == 0:
                continue
            take = idx[rng.permutation(len(idx))[: min(shots, len(idx))]]
            tr_idx.extend(take.tolist())
        tr_set = set(tr_idx)
        te_idx = np.array([i for i in range(len(y)) if i not in tr_set])
        if len(tr_idx) == 0 or len(te_idx) == 0:
            continue
        r = logistic_probe(X[tr_idx], y[tr_idx], X[te_idx], y[te_idx],
                           n_classes=n_classes, steps=steps, lr=lr,
                           seed=seed + rep)
        accs.append(r["acc"])
    return {"acc": float(np.mean(accs)) if accs else float("nan"),
            "accs": accs}
