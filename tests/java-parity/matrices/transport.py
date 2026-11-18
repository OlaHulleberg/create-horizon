"""MIT input matrix from Delete228/create-horizon/tools/transport_oracle.py."""
import itertools
import random

def cases():
    cases = []
    for (axis, other, x, y, z) in itertools.product(range(3), range(3), range(-5, 6), range(-5, 6), range(-5, 6)):
        cases.append(f'C {axis} {other} {x} {y} {z} 16 16 0 0 20')
    for (axis, x, y, z, r1, r2, flags, space) in itertools.product(range(3), [0, 3, 14, 19, 20], [0, 3, 14, 19, 20], [0], [0, 16, -32], [0, 64, -16], [0, 1, 2, 4, 8], range(4)):
        cases.append(f'C {axis} {axis} {x} {y} {z} {r1} {r2} {flags} {space} 20')
    for (rpm, facing) in itertools.product([x / 8 for x in range(-4096, 4097)], range(2, 6)):
        cases.append(f'M {rpm} {facing}')
    for (rpm, facing, slope) in itertools.product([0.0, -0.0, -256, -0.5, 16, 128, 256, 8192], range(2, 6), range(5)):
        cases.append(f'E {rpm} {facing} {slope}')
    rng = random.Random(3955)
    for _ in range(5000):
        (rpm, facing, positive, segment, side) = (rng.choice([0, -256, 16, 256]), rng.randrange(2, 6), rng.randrange(2), rng.randrange(5), rng.randrange(6))
        (position, inserted, origin) = (rng.randrange(-8, 88) / 16, rng.randrange(5), rng.randrange(6))
        cases.append(f'I {rpm} {facing} {positive} {segment} {side} {position} {inserted} {origin}')
    for (segment, p) in itertools.product(range(5), [-0.001, 0, 0.001, 0.5, 1, 1.001, 2, 4.999, 5]):
        cases.append(f'G {segment} 3 {p} 1.0 2.0')
    for _ in range(1800):
        (rpm, facing, slope, length, positive, ending) = (rng.choice([0, -256, -0.5, 16, 128, 256, 8192]), rng.randrange(2, 6), rng.randrange(3), rng.choice([2, 3, 5]), rng.randrange(2), rng.randrange(4))
        (ticks, room, pseg, pmode) = (rng.choice([1, 2, 7, 35]), rng.choice([0, 1, 16, 64]), rng.randrange(length), rng.randrange(4))
        positions = sorted([rng.randrange(length * 16 + 1) / 16 for _ in range(rng.randrange(1, 5))], reverse=bool(positive))
        items = ' '.join((f'{rng.choice([0, 1, 16, 64])} {pos} {rng.randrange(360)} {rng.randrange(-8, 9) / 8} {rng.randrange(2)} {rng.randrange(2)}' for pos in positions))
        cases.append(f'T {rpm} {facing} {slope} {length} {positive} {ending} {ticks} {room} {pseg} {pmode} {len(positions)} {items}')
    return cases
