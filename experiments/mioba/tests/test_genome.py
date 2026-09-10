import json
import random

from experiments.mioba.genome.hashing import canonical_json, genome_hash
from experiments.mioba.genome.mutation import mutate
from experiments.mioba.genome.schema import Genome, fba0_genome


def test_hash_stable_across_key_order():
    g = fba0_genome(seed=1)
    d = g.to_dict()
    reordered = dict(reversed(list(d.items())))
    g2 = Genome.from_dict(reordered)
    assert genome_hash(g) == genome_hash(g2)


def test_hash_ignores_created_at_and_genome_id():
    g = fba0_genome(seed=1)
    h1 = genome_hash(g)
    g.created_at = "2020-01-01T00:00:00+00:00"
    g.genome_id = "b2b:other"
    assert genome_hash(g) == h1
    assert h1.startswith("b2b:")


def test_different_mutation_different_hash():
    base = fba0_genome(seed=1)
    seen = set()
    rng = random.Random(7)
    for i in range(10):
        child = mutate(base, rng, birth_index=i, generation=1)
        seen.add(child.genome_id)
    assert len(seen) > 1


def test_json_roundtrip():
    rng = random.Random(3)
    base = fba0_genome()
    child = mutate(base, rng, birth_index=1, generation=1)
    text = child.to_json()
    back = Genome.from_json(text)
    assert back == child
    assert json.loads(text)["genome_id"] == child.genome_id


def test_canonical_json_sorted_compact():
    assert canonical_json({"b": 1, "a": {"y": 2, "x": 1}}) == \
        '{"a":{"x":1,"y":2},"b":1}'
