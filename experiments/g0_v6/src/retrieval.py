"""Label-free temporal retrieval for CPC health and generalization tests.

The only positive relation used here is the known episode/time correspondence:
the future target is the observation from the same episode at the requested
offset. No semantic or generated labels are loaded by this module.
"""

import numpy as np
import torch


def _normalise(x):
    return x / np.maximum(np.linalg.norm(x, axis=-1, keepdims=True), 1e-12)


def _rank_of_positive(scores, positive_index):
    positive = float(scores[positive_index])
    greater = np.count_nonzero(scores > positive)
    ties = np.count_nonzero(scores == positive) - 1
    # Mid-rank handles exact ties without depending on candidate ordering.
    return 1.0 + greater + 0.5 * ties


def retrieval_from_latent(latent, horizons, stride=4, max_episodes=128, hard_negatives=True):
    """Retrieve the future latent using the current latent as the query."""
    z = np.asarray(latent, dtype=np.float64)
    if z.ndim != 3 or not np.isfinite(z).all():
        raise ValueError("latent must be finite [N,T,Z]")
    z = z[:max_episodes]
    n_episodes, length = z.shape[:2]
    by_horizon = {}
    all_ranks = []
    for horizon in tuple(int(x) for x in horizons):
        if horizon < 1 or horizon >= length:
            continue
        ranks = []
        for time in range(0, length - horizon, max(1, int(stride))):
            query = _normalise(z[:, time])
            # Evaluate one query per episode. The first block is the exact
            # target-time candidate set; subsequent blocks are hard temporal
            # negatives for the corresponding query episode.
            exact = _normalise(z[:, time + horizon])
            for episode in range(n_episodes):
                scores = [float(query[episode] @ exact[j]) for j in range(n_episodes)]
                # The candidate is the episode at the correct future time.
                if hard_negatives:
                    for other in range(n_episodes):
                        for offset in (-1, 1):
                            candidate_time = time + horizon + offset
                            if 0 <= candidate_time < length:
                                value = _normalise(z[other:other + 1, candidate_time])[0]
                                scores.append(float(query[episode] @ value))
                # The query is represented by the current latent. This is the
                # fixed, no-fit retrieval baseline for unseen horizons and
                # analytic controls.
                if hard_negatives:
                    positive_score = float(query[episode] @ _normalise(z[episode:episode + 1, time + horizon])[0])
                    # Rebuild the score at its true location because exact
                    # candidates are scored against their own target vectors.
                    scores[episode] = positive_score
                ranks.append(_rank_of_positive(np.asarray(scores), episode))
        if ranks:
            ranks_array = np.asarray(ranks, dtype=np.float64)
            by_horizon[str(horizon)] = {
                "recall_at_1": float(np.mean(ranks_array <= 1.0)),
                "recall_at_5": float(np.mean(ranks_array <= 5.0)),
                "mrr": float(np.mean(1.0 / ranks_array)),
                "mean_rank": float(ranks_array.mean()),
                "n_queries": int(len(ranks_array)),
                "candidate_count": int(n_episodes + (2 * n_episodes if hard_negatives else 0)),
            }
            all_ranks.extend(ranks)
    if not all_ranks:
        raise ValueError("no valid retrieval queries")
    ranks = np.asarray(all_ranks, dtype=np.float64)
    return {
        "recall_at_1": float(np.mean(ranks <= 1.0)),
        "recall_at_5": float(np.mean(ranks <= 5.0)),
        "mrr": float(np.mean(1.0 / ranks)),
        "mean_rank": float(ranks.mean()),
        "n_queries": int(len(ranks)),
        "horizons": [int(x) for x in horizons if str(int(x)) in by_horizon],
        "by_horizon": by_horizon,
        "query": "current latent; same episode and future time positive",
        "negatives": "all episodes at exact target time plus same-episode adjacent target times",
    }


@torch.no_grad()
def _encode_model(model, observations, device, batch_size=64):
    model.eval()
    encoded = []
    for start in range(0, len(observations), batch_size):
        batch = torch.from_numpy(np.asarray(observations[start:start + batch_size], dtype=np.float32)).to(device)
        encoded.append(model.encoder(batch).cpu().numpy())
    return np.concatenate(encoded)


@torch.no_grad()
def model_retrieval(model, observations, device, horizons, stride=4, max_episodes=128, predictor=False):
    """Run retrieval with a CPC predictor for trained horizons when requested."""
    z = _encode_model(model, observations, device)
    if not predictor:
        return retrieval_from_latent(z, horizons, stride, max_episodes)
    z = z[:max_episodes]
    query_results = []
    by_horizon = {}
    for horizon in tuple(int(x) for x in horizons):
        if horizon < 1 or horizon >= z.shape[1]:
            continue
        try:
            predictor_module = model.predictor_for(horizon)
        except ValueError:
            continue
        predicted = []
        for start in range(0, len(z), 64):
            values = torch.from_numpy(z[start:start + 64, :-horizon]).to(device)
            predicted.append(predictor_module(values).cpu().numpy())
        predicted = np.concatenate(predicted)
        n_episodes, length = z.shape[:2]
        ranks = []
        for time in range(0, length - horizon, max(1, int(stride))):
            exact_targets = _normalise(z[:, time + horizon])
            for episode in range(n_episodes):
                query = _normalise(predicted[episode, time:time + 1])[0]
                scores = [float(query @ exact_targets[j]) for j in range(n_episodes)]
                if n_episodes > 1:
                    for other in range(n_episodes):
                        for offset in (-1, 1):
                            candidate_time = time + horizon + offset
                            if 0 <= candidate_time < length:
                                target = _normalise(z[other:other + 1, candidate_time])[0]
                                scores.append(float(query @ target))
                ranks.append(_rank_of_positive(np.asarray(scores), episode))
        ranks = np.asarray(ranks, dtype=np.float64)
        by_horizon[str(horizon)] = {
            "recall_at_1": float(np.mean(ranks <= 1.0)),
            "recall_at_5": float(np.mean(ranks <= 5.0)),
            "mrr": float(np.mean(1.0 / ranks)),
            "mean_rank": float(ranks.mean()),
            "n_queries": int(len(ranks)),
            "candidate_count": int(n_episodes + 2 * n_episodes),
        }
        query_results.extend(ranks.tolist())
    if not query_results:
        raise ValueError("no predictor retrieval queries")
    ranks = np.asarray(query_results, dtype=np.float64)
    return {
        "recall_at_1": float(np.mean(ranks <= 1.0)),
        "recall_at_5": float(np.mean(ranks <= 5.0)),
        "mrr": float(np.mean(1.0 / ranks)),
        "mean_rank": float(ranks.mean()),
        "n_queries": int(len(ranks)),
        "horizons": [int(x) for x in horizons if str(int(x)) in by_horizon],
        "by_horizon": by_horizon,
        "query": "CPC predictor(current latent); same episode and future time positive",
        "negatives": "all episodes at exact target time plus same-episode adjacent target times",
    }
