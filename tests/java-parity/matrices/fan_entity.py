"""MIT input matrix from Delete228/create-horizon/tools/fan_entity_checks.py."""
import itertools
import random

def cases():
    cases = ['E ' + ' '.join(map(str, values)) for values in itertools.product(range(4), range(6), range(2), range(2), [-20, -10, -1, *range(0, 111), 150])]
    rng = random.Random(851462)
    for (rpm, axis, sign, sneak, creative, flying, spectator, alive) in itertools.product([1, 16, 32, 128, 256, 512], range(3), [-1, 1], range(2), range(2), range(2), range(2), range(2)):
        point = [rng.uniform(-30, 30) for _ in range(3)]
        velocity = [rng.uniform(-8, 8) for _ in range(3)]
        cases.append('M ' + ' '.join(map(str, [rpm, rng.uniform(0.25, 20), axis, sign, sneak, creative, flying, spectator, alive, *point, *velocity])))
    return cases
