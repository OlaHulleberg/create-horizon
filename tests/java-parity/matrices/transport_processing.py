"""MIT input matrix from tools/transport_processing_oracle.py in Delete228/create-horizon."""
import itertools
import random

def cases():
    cases = []
    for (positive, position, distance, mode, rounds) in itertools.product([0, 1], [-2.75, -0.5, 0, 2.5], [0, 0.5, 0.51, 1], range(7), [1, 2]):
        items = f'1 8 {position - 0.5} 19 0 1 8 {position} 43 0 2 1 {position + 0.5} 91 0 9 3 {position} 16 1'
        cases.append(f'{positive} {position} {distance} {mode} {rounds} 4 {items}')
    rng = random.Random(3955)
    for _ in range(1000):
        position = rng.randrange(-64, 97) / 16
        items = ' '.join((f'{rng.randrange(3)} {rng.randrange(1, 65)} {rng.randrange(-64, 97) / 16} {rng.randrange(360)} {rng.randrange(2)}' for _ in range(5)))
        cases.append(f'{rng.randrange(2)} {position} {rng.choice([0.5, 0.51, 1, 10])} {rng.randrange(7)} {rng.randrange(1, 4)} 5 {items}')
    return cases
