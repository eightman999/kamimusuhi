"""Body0 — minimal 2D virtual body + closed sensorimotor loop
(Body0 §20-§52).

State: x, y, heading (rad), v_lin, v_ang. Environment: a point target,
a circular boundary, optional obstacles. Task: orient toward the
target (heading error → 0), optionally approach it.

Data path (audited, §25/§45):

    env → sensor encode (MODEL_INFERENCE Poisson rates)
        → BANC sensory/afferent entities (CURATED_ANNOTATION port)
        → host CNS (active_fly_v1_1 or passive)
        → descending/motor entities split by `side`
          (CURATED_ANNOTATION port)
        → actuator model (MODEL_INFERENCE): v_ang = k*(L−R),
          v_lin = kf*(L+R)/2
        → body → env

There is deliberately no sensor→body or graft→body shortcut.

All models are marked: body/actuator/sensor = MODEL_INFERENCE;
host port identity = CURATED_ANNOTATION (dataset `super_class`,
`flow_class`, `side` columns).
"""
from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

BODY0_VERSION = "body0-v0"


# ------------------------------------------------------------------ #
# body + environment                                                  #
# ------------------------------------------------------------------ #
@dataclass
class BodyState:
    x: float = 0.0
    y: float = 0.0
    heading: float = 0.0          # rad, CCW+
    v_lin: float = 0.0            # units/s
    v_ang: float = 0.0            # rad/s


@dataclass
class Environment:
    """2D plane: point target + circular boundary + obstacles."""
    target: tuple[float, float] = (10.0, 0.0)
    boundary_r: float = 30.0
    obstacles: tuple = ()
    relocations: tuple = ()       # [(t_s, (x, y)), ...] deterministic

    def target_at(self, t_s: float) -> tuple[float, float]:
        tgt = self.target
        for t0, xy in self.relocations:
            if t_s >= t0:
                tgt = xy
        return tgt


def bearing(body: BodyState, target) -> float:
    """Signed angle from heading to target, in [-pi, pi]."""
    dx, dy = target[0] - body.x, target[1] - body.y
    return math.atan2(dy, dx) - body.heading


def wrap_pi(a: float) -> float:
    return (a + math.pi) % (2 * math.pi) - math.pi


# ------------------------------------------------------------------ #
# sensor encoding (§26-§27): bearing → bilateral Poisson rates        #
# ------------------------------------------------------------------ #
def encode_sensory(phi: float, intensity: float = 1.0,
                   base_hz: float = 5.0, gain_hz: float = 60.0):
    """Target on the left (phi>0) drives the LEFT afferent population.
    Returns (rate_left_hz, rate_right_hz). Mapping: MODEL_INFERENCE."""
    half = 0.5 * gain_hz * intensity
    return (base_hz + half * max(0.0, phi / math.pi),
            base_hz + half * max(0.0, -phi / math.pi))


# ------------------------------------------------------------------ #
# motor decode (§24, §29): bilateral population rates → velocity      #
# ------------------------------------------------------------------ #
def decode_motor(rate_left: float, rate_right: float,
                 k_ang: float = 0.03, k_lin: float = 0.02,
                 baseline: tuple[float, float] = (0.0, 0.0)):
    """Left-population activity turns left (heading +=), symmetric
    drive moves forward. ``baseline`` (bL, bR) subtracts the
    population's spontaneous rate measured in a no-input probe —
    anatomical asymmetry otherwise adds a constant turning bias.
    Actuator model: MODEL_INFERENCE."""
    dL = max(0.0, rate_left - baseline[0])
    dR = max(0.0, rate_right - baseline[1])
    v_ang = k_ang * (dL - dR)
    v_lin = k_lin * 0.5 * (dL + dR)
    return v_lin, v_ang


def measure_motor_baseline(backend, ports: dict,
                           probe_ms: float = 200.0
                           ) -> tuple[float, float]:
    """Spontaneous L/R population rates under zero sensory drive —
    subtracted by decode_motor (MODEL_INFERENCE calibration step).
    Leaves the backend reset afterwards."""
    backend.reset()
    backend.set_inputs({"rates_hz": {}})
    backend.run(probe_ms)
    sc = backend.spike_counts[0].float()
    bL = float(sc[ports["motor_left"]].sum()) / (probe_ms / 1000.0) \
        if ports["motor_left"] else 0.0
    bR = float(sc[ports["motor_right"]].sum()) / (probe_ms / 1000.0) \
        if ports["motor_right"] else 0.0
    backend.reset()
    return bL, bR


def oracle_control(phi: float, kp: float = 3.0, v_lin: float = 0.5):
    """Condition A baseline: proportional heading controller."""
    return v_lin, kp * phi


def step_body(body: BodyState, v_lin: float, v_ang: float,
              dt_s: float) -> None:
    body.v_lin, body.v_ang = v_lin, v_ang
    body.heading = wrap_pi(body.heading + v_ang * dt_s)
    body.x += v_lin * math.cos(body.heading) * dt_s
    body.y += v_lin * math.sin(body.heading) * dt_s


# ------------------------------------------------------------------ #
# sensorimotor circuit selection (§28-§30)                            #
#                                                                     #
# _pick_circuit (G0.1) samples afferents + descending entities and    #
# 1-hop partners — convenient for graft tests, but a random 240-cell  #
# subgraph almost never contains a real sensory→descending path       #
# (measured: 0/52 sensory entities reach any motor entity). A closed  #
# loop needs real paths, so Body0 selects the circuit connectivity-   #
# informed: descending entities preferentially reachable from one     #
# sensory side (dL < dR), plus the shortest BFS paths connecting      #
# same-side sensory sources to them. Entity *identity* stays          #
# CURATED_ANNOTATION; the *grouping* (which descending pool drives    #
# which turn direction) is MODEL_INFERENCE and documented as such.    #
# ------------------------------------------------------------------ #
def _csr(conn, pos, N):
    pre = conn["pre_idx"].map(pos).to_numpy()
    post = conn["post_idx"].map(pos).to_numpy()
    o = np.argsort(pre, kind="stable")
    pre_s, post_s = pre[o], post[o]
    ip = np.zeros(N + 1, np.int64)
    np.add.at(ip, pre_s + 1, 1)
    ip = np.cumsum(ip)
    o2 = np.argsort(post, kind="stable")
    post2, pre2 = post[o2], pre[o2]
    ipr = np.zeros(N + 1, np.int64)
    np.add.at(ipr, post2 + 1, 1)
    ipr = np.cumsum(ipr)
    return (ip, post_s), (ipr, pre2)


def _bfs_dist(sources, depth, ptr, ch):
    d = np.full(len(ptr) - 1, -1, np.int32)
    if not len(sources):
        return d
    d[list(sources)] = 0
    fr = np.asarray(list(sources))
    for k in range(1, depth + 1):
        nxt = np.concatenate([ch[ptr[u]:ptr[u + 1]] for u in fr]) \
            if len(fr) else fr
        nxt = nxt[d[nxt] < 0]
        if not len(nxt):
            break
        nxt = np.unique(nxt)
        d[nxt] = k
        fr = nxt
    return d


def pick_sensorimotor_circuit(ent, conn, n_circuit: int, seed: int,
                              depth: int = 3, n_motor: int = 8,
                              max_paths_per_motor: int = 4) -> dict:
    """Connectivity-informed sensorimotor subgraph (see banner above).

    Returns entity_idx lists: ``circuit``, ``drivers`` (sensory port
    entities per side), ``targets`` (motor pools), ``sensory_left`` /
    ``sensory_right`` / ``motor_left`` / ``motor_right``.
    """
    idx = np.asarray(ent["entity_idx"])
    pos = {int(e): i for i, e in enumerate(idx)}
    N = len(idx)
    (ip, post_s), (ipr, pre2) = _csr(conn, pos, N)
    sens_mask = ent["super_class"].isin(
        {"sensory", "visual_projection", "sensory_ascending"}
    ).to_numpy()
    mot_mask = ent["super_class"].isin(
        {"descending", "motor"}).to_numpy()
    side = ent["side"].fillna("?").to_numpy()
    sensL = np.nonzero(sens_mask & (side == "left"))[0]
    sensR = np.nonzero(sens_mask & (side == "right"))[0]
    dL = _bfs_dist(sensL, depth, ip, post_s)
    dR = _bfs_dist(sensR, depth, ip, post_s)
    left_pool = np.nonzero(mot_mask & (dL >= 0)
                           & ((dR < 0) | (dL < dR)))[0]
    right_pool = np.nonzero(mot_mask & (dR >= 0)
                            & ((dL < 0) | (dR < dL)))[0]
    if not len(left_pool) or not len(right_pool):
        raise RuntimeError(
            "no asymmetric sensory->motor pools at this depth")

    def _pick_motors(pool, d_pref, want_side):
        # closest-coupled first, preferring matching anatomical side
        ranked = sorted(pool.tolist(),
                        key=lambda m: (int(d_pref[m]),
                                       side[m] != want_side,
                                       int(idx[m])))
        return ranked[:n_motor]

    motL = _pick_motors(left_pool, dL, "left")
    motR = _pick_motors(right_pool, dR, "right")

    # ---- shortest paths: layered reverse BFS per motor entity ----- #
    def _paths_to(motor, sens_src):
        layers = [np.array([motor])]
        seen = np.zeros(N, bool)
        seen[motor] = True
        for k in range(1, depth + 1):
            fr = layers[-1]
            nxt = np.concatenate(
                [pre2[ipr[u]:ipr[u + 1]] for u in fr])
            nxt = nxt[~seen[nxt]]
            if not len(nxt):
                break
            nxt = np.unique(nxt)
            seen[nxt] = True
            layers.append(nxt)
        hits = []
        for k, lay in enumerate(layers):
            h = lay[sens_mask[lay] & np.isin(lay, list(sens_src))]
            hits.extend((int(s), k) for s in h)
            if len(hits) >= max_paths_per_motor or k == len(layers) - 1:
                break
        layer_of = {}
        for k, lay in enumerate(layers):
            for u in lay:
                layer_of[int(u)] = k
        paths = []
        for s, k in hits[:max_paths_per_motor]:
            path = [s]
            cur = s
            for j in range(k - 1, -1, -1):
                nb = post_s[ip[cur]:ip[cur + 1]]
                nb = [int(v) for v in nb if layer_of.get(int(v)) == j]
                if not nb:
                    break
                cur = min(nb)
                path.append(cur)
            paths.append(path)
        return paths

    sens_port_L, sens_port_R = set(), set()
    path_entities = set(motL) | set(motR)
    for m in motL:
        for p in _paths_to(m, sensL):
            path_entities.update(p)
            sens_port_L.add(p[0])
    for m in motR:
        for p in _paths_to(m, sensR):
            path_entities.update(p)
            sens_port_R.add(p[0])

    core = path_entities | set(motL) | set(motR)
    # fill remainder with strongest 1-hop partners (as _pick_circuit)
    part = conn[conn["pre_idx"].isin(
        {int(idx[u]) for u in core}) | conn["post_idx"].isin(
        {int(idx[u]) for u in core})]
    part = part.sort_values("anatomical_count", ascending=False)
    ring = set(part["pre_idx"]).union(part["post_idx"]) \
        - {int(idx[u]) for u in core}
    circuit = sorted({int(idx[u]) for u in core}
                     | set(list(ring)[: max(0, n_circuit - len(core))]))
    return {
        "circuit": circuit,
        "drivers": sorted(int(idx[u]) for u in
                          sens_port_L | sens_port_R),
        "targets": sorted(int(idx[m]) for m in motL + motR),
        "sensory_left": sorted(int(idx[u]) for u in sens_port_L),
        "sensory_right": sorted(int(idx[u]) for u in sens_port_R),
        "motor_left": sorted(int(idx[m]) for m in motL),
        "motor_right": sorted(int(idx[m]) for m in motR),
        "n_path_entities": len(core),
        "selection": {
            "rule": "descending/motor entities with asymmetric "
                    "sensory reach (dL<dR -> left-turn pool), shortest "
                    "paths to same-side sensory sources",
            "depth": depth, "n_motor_per_side": n_motor,
            "identity_provenance": "CURATED_ANNOTATION",
            "grouping_provenance": "MODEL_INFERENCE",
        },
    }


# ------------------------------------------------------------------ #
# port selection (§28-§30): annotation-derived, never random          #
# ------------------------------------------------------------------ #
def select_ports(ent, circuit: list[int], comp_of: dict,
                 ais_name: str = "AXON_INITIATION_ZONE",
                 picked: dict | None = None) -> dict:
    """Pick host port nodes from CURATED_ANNOTATION columns.

    sensory port: super_class in {sensory, visual_projection,
                  sensory_ascending} — split by `side`; drive node is
                  the entity SOMA (synaptic bombardment via set_inputs).
    motor port:   super_class in {descending, motor} — split by
                  `side` (left pop → turn left); read node is the AIS
                  when present (spike emission site) else SOMA.

    ``picked``: optional entity lists from
    :func:`pick_sensorimotor_circuit` (connectivity-informed grouping;
    keys sensory_left/right + motor_left/right as entity_idx). When
    given, only those entities are mapped to nodes — annotation still
    defines identity, connectivity defines grouping.

    Entities without a mapped compartment are skipped (recorded in the
    port manifest).
    """
    def _pick(e, names):
        nodes = comp_of.get(int(e)) or {}
        for nm in names:
            if nm in nodes:
                return nodes[nm]
        return None

    def pop_entities(es, names):
        out = []
        for e in es:
            nd = _pick(e, names)
            if nd is not None:
                out.append(nd)
        return sorted(out)

    if picked is not None:
        sL, sR = picked["sensory_left"], picked["sensory_right"]
        mL, mR = picked["motor_left"], picked["motor_right"]
        return {
            "sensory_left": pop_entities(sL, ("SOMA",)),
            "sensory_right": pop_entities(sR, ("SOMA",)),
            "motor_left": pop_entities(mL, (ais_name, "SOMA")),
            "motor_right": pop_entities(mR, (ais_name, "SOMA")),
            "sensory_entities": sorted(sL + sR),
            "motor_entities": sorted(mL + mR),
        }
    sub = ent[ent["entity_idx"].isin(circuit)]
    sens_cls = {"sensory", "visual_projection", "sensory_ascending"}
    mot_cls = {"descending", "motor"}

    def pop(df, names):
        return pop_entities(df["entity_idx"].tolist(), names)

    sens = sub[sub["super_class"].isin(sens_cls)]
    mot = sub[sub["super_class"].isin(mot_cls)]
    return {
        "sensory_left": pop(sens[sens["side"] == "left"], ("SOMA",)),
        "sensory_right": pop(sens[sens["side"] == "right"], ("SOMA",)),
        "motor_left": pop(mot[mot["side"] == "left"],
                          (ais_name, "SOMA")),
        "motor_right": pop(mot[mot["side"] == "right"],
                           (ais_name, "SOMA")),
        "sensory_entities": sorted(int(e) for e in
                                 sens["entity_idx"]),
        "motor_entities": sorted(int(e) for e in
                                 mot["entity_idx"]),
    }


def port_manifest(ports: dict) -> dict:
    """§46: port audit record for the run manifest."""
    return {
        "version": BODY0_VERSION,
        "sensory_port": {
            "entities": ports["sensory_entities"],
            "n_nodes_left": len(ports["sensory_left"]),
            "n_nodes_right": len(ports["sensory_right"]),
            "selection": "super_class in {sensory, visual_projection, "
                         "sensory_ascending}, split by side; "
                         "drive at SOMA",
            "identity_provenance": "CURATED_ANNOTATION",
            "encode_provenance": "MODEL_INFERENCE",
        },
        "motor_port": {
            "entities": ports["motor_entities"],
            "n_nodes_left": len(ports["motor_left"]),
            "n_nodes_right": len(ports["motor_right"]),
            "read_compartment": "AIS if present else SOMA "
                                "(spike emission site)",
            "selection": "super_class in {descending, motor}, "
                         "split by side",
            "identity_provenance": "CURATED_ANNOTATION",
            "decode_provenance": "MODEL_INFERENCE",
        },
        "body_model": "2D unicycle — MODEL_INFERENCE",
        "biomechanics": "simplified",
        "no_shortcut": "sensor→CNS→descending→body only; "
                       "no direct sensor→body or graft→body path",
    }


# ------------------------------------------------------------------ #
# closed-loop episode (§32)                                           #
# ------------------------------------------------------------------ #
def run_episode(backend, ports: dict, env: Environment,
                duration_s: float, tick_ms: float = 20.0,
                sensory_gain_hz: float = 60.0,
                sensory_base_hz: float = 5.0,
                k_ang: float = 0.03, k_lin: float = 0.02,
                decode_window_s: float = 0.1,
                motor_baseline: tuple[float, float] = (0.0, 0.0),
                initial_heading: float = 1.2,
                heading_perturb: tuple | None = None,
                sensory_ablation: bool = False,
                motor_ablation: bool = False,
                record: bool = True) -> dict:
    """One closed-loop episode. ``backend`` is already initialized
    (graft conditions baked into its phenotype); we drive the sensory
    port each control tick and decode the motor port.

    ``heading_perturb``: optional (t_s, delta_rad) deterministic kick.
    """
    body = BodyState(heading=initial_heading)
    dt = tick_ms / 1000.0
    n_ticks = int(duration_s / dt)
    prev_counts = None
    from collections import deque
    win = max(1, int(round(decode_window_s / dt)))
    d_hist: deque = deque(maxlen=win)
    tr = {"t": [], "x": [], "y": [], "heading": [],
          "bearing": [], "rate_L": [], "rate_R": [],
          "sens_L_hz": [], "sens_R_hz": [],
          "motor_L_hz": [], "motor_R_hz": []}
    first_move_t = None
    first_motor_t = None   # first descending/motor population spike
    first_sens_t = None    # first nonzero sensory drive tick
    collisions = 0
    energy = 0.0
    path = 0.0
    for k in range(n_ticks):
        t_s = k * dt
        tgt = env.target_at(t_s)
        phi = wrap_pi(bearing(body, tgt))
        intensity = min(1.0, 10.0 /
                        max(math.dist((body.x, body.y), tgt), 1e-6))
        if sensory_ablation:
            sL = sR = 0.0
        else:
            sL, sR = encode_sensory(phi, intensity,
                                    sensory_base_hz, sensory_gain_hz)
        drive = {str(nd): sL for nd in ports["sensory_left"]}
        drive.update({str(nd): sR for nd in ports["sensory_right"]})
        backend.set_inputs({"rates_hz": drive})
        backend.run(tick_ms)
        if heading_perturb and abs(t_s - heading_perturb[0]) < dt / 2:
            body.heading = wrap_pi(body.heading +
                                   heading_perturb[1])
        # decode motor population rates from this tick's spikes
        cur = backend.spike_counts[0]
        if prev_counts is None:
            prev_counts = cur.clone()
        d = (cur - prev_counts).float()
        prev_counts = cur.clone()
        d_hist.append(d)
        if first_sens_t is None and (sL > 0 or sR > 0):
            first_sens_t = t_s
        if motor_ablation:
            mL = mR = 0.0
        else:
            # windowed decode — per-tick spike counts are too coarse
            # (~1 spike/tick/pop) for stable control (MODEL_INFERENCE)
            dw = sum(d_hist) / (len(d_hist) * dt)
            mL = float(dw[ports["motor_left"]].sum()) \
                if ports["motor_left"] else 0.0
            mR = float(dw[ports["motor_right"]].sum()) \
                if ports["motor_right"] else 0.0
            if first_motor_t is None and (mL > 0 or mR > 0):
                first_motor_t = t_s
        v_lin, v_ang = decode_motor(mL, mR, k_ang, k_lin,
                                  motor_baseline)
        px, py = body.x, body.y
        step_body(body, v_lin, v_ang, dt)
        path += math.dist((px, py), (body.x, body.y))
        energy += abs(v_ang) * dt
        if math.dist((body.x, body.y), (0.0, 0.0)) > env.boundary_r:
            collisions += 1
            body.x *= 0.9 * env.boundary_r / \
                math.dist((body.x, body.y), (0, 0))
            body.y *= 0.9 * env.boundary_r / \
                math.dist((body.x, body.y), (0, 0))
        if first_move_t is None and (abs(v_ang) > 1e-6
                                     or abs(v_lin) > 1e-6):
            first_move_t = t_s
        if record:
            for key, val in (("t", t_s), ("x", body.x),
                             ("y", body.y), ("heading", body.heading),
                             ("bearing", phi), ("rate_L", mL),
                             ("rate_R", mR), ("sens_L_hz", sL),
                             ("sens_R_hz", sR), ("motor_L_hz", mL),
                             ("motor_R_hz", mR)):
                tr[key].append(float(val))
    errs = [abs(wrap_pi(b)) for b in tr["bearing"][n_ticks // 2:]]
    mean_abs_err = float(np.mean(errs)) if errs else float("nan")
    thr = math.radians(15.0)
    t_orient = next((tr["t"][i] for i, b in enumerate(tr["bearing"])
                     if abs(wrap_pi(b)) < thr), None)
    return {
        "metrics": {
            "mean_abs_heading_err_rad_tail": mean_abs_err,
            "time_to_15deg_s": t_orient,
            "distance_to_target_end": float(
                math.dist((body.x, body.y),
                          env.target_at(duration_s))),
            "path_length": path,
            "control_energy": energy,
            "collision_count": collisions,
            "first_motion_latency_s": first_move_t,
            # §41 sensorimotor latency chain
            "first_sensory_drive_s": first_sens_t,
            "first_motor_spike_s": first_motor_t,
            "sensor_to_motor_latency_s": (
                first_motor_t - first_sens_t
                if first_motor_t is not None
                and first_sens_t is not None else None),
            "motor_to_motion_latency_s": (
                first_move_t - first_motor_t
                if first_move_t is not None
                and first_motor_t is not None else None),
        },
        "trace": tr if record else None,
        "final_body": {"x": body.x, "y": body.y,
                       "heading": body.heading},
        "numerical_failure": getattr(backend, "_num_failure", None),
    }


def run_oracle(env: Environment, duration_s: float,
               tick_ms: float = 20.0, initial_heading: float = 1.2,
               kp: float = 3.0) -> dict:
    """Condition A: proportional-controller baseline, no CNS."""
    body = BodyState(heading=initial_heading)
    dt = tick_ms / 1000.0
    n = int(duration_s / dt)
    tr = {"t": [], "heading": [], "bearing": []}
    for k in range(n):
        t_s = k * dt
        tgt = env.target_at(t_s)
        phi = wrap_pi(bearing(body, tgt))
        v_lin, v_ang = oracle_control(phi, kp)
        step_body(body, v_lin, v_ang, dt)
        tr["t"].append(t_s); tr["heading"].append(body.heading)
        tr["bearing"].append(phi)
    errs = [abs(wrap_pi(b)) for b in tr["bearing"][n // 2:]]
    thr = math.radians(15.0)
    return {"metrics": {
        "mean_abs_heading_err_rad_tail": float(np.mean(errs)),
        "time_to_15deg_s": next(
            (tr["t"][i] for i, b in enumerate(tr["bearing"])
             if abs(wrap_pi(b)) < thr), None)},
        "trace": tr}
