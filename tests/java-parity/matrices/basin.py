"""Basin/mixer cases from Delete228/create-horizon/tools/basin_oracle.py (MIT)."""
import itertools
import random

def encode(case):
    out = ['A', case['test'], case['basin'], case['heat'], case['required'], case['mode'], case['item_dest'], case['fluid_dest'], case['items_present'], case['fluids_present'], case['valid']]
    out += [len(case['ingredients']), *case['ingredients'], len(case['fluid_ingredients'])]
    for entry in case['fluid_ingredients']:
        out += list(entry)
    out += [len(case['outputs'])]
    for entry in case['outputs']:
        out += list(entry)
    out += [len(case['fluid_outputs'])]
    for entry in case['fluid_outputs']:
        out += list(entry)
    for entry in case['items'] + case['fluids']:
        out += list(entry)
    return ' '.join(map(str, out))

def fixture():
    return {'test': 0, 'basin': 1, 'heat': 0, 'required': 0, 'mode': 0, 'item_dest': 0, 'fluid_dest': 0, 'items_present': 1, 'fluids_present': 1, 'valid': 1, 'ingredients': [], 'fluid_ingredients': [], 'outputs': [], 'fluid_outputs': [], 'items': [(-1, 0, 64)] * 18, 'fluids': [(-1, 0)] * 4}

def cases():
    result = []
    named = {}
    c = fixture()
    c['ingredients'] = [1 << 1]
    c['items'][0] = (1, 1, 64)
    c['items'][9] = (2, 60, 64)
    c['outputs'] = [(2, 3, 64, 1), (2, 3, 64, 1)]
    result.append(encode(c))
    named['source_overbooking_60_plus_3_plus_3'] = 0
    c = fixture()
    c['fluids'][0] = (1, 500)
    c['fluid_ingredients'] = [(1, 300), (1, 300)]
    named['source_partial_drain_500_for_300_plus_300'] = len(result)
    result.append(encode(c))
    c = fixture()
    c['fluids'][0] = (1, 700)
    c['fluid_outputs'] = [(1, 200), (1, 200)]
    named['source_fluid_overbooking_700_plus_200_plus_200'] = len(result)
    result.append(encode(c))
    c = fixture()
    c['fluid_outputs'] = [(1, 1500)]
    named['new_fluid_spans_both_empty_tanks'] = len(result)
    result.append(encode(c))
    c = fixture()
    c['fluids'][0] = (1, 1000)
    c['fluids'][1] = (1, 500)
    c['fluid_outputs'] = [(1, 100)]
    named['existing_full_matching_tank_stops_search'] = len(result)
    result.append(encode(c))
    c = fixture()
    c['items'][0] = (1, 1, 64)
    c['items'][1] = (2, 1, 64)
    c['ingredients'] = [6, 2]
    named['greedy_ingredient_order_does_not_backtrack'] = len(result)
    result.append(encode(c))
    c = fixture()
    c['items'][0] = (9, 2, 64)
    c['items'][9] = (2, 1, 64)
    c['ingredients'] = [4, 512, 512]
    named['combined_output_consumption_and_slot_order_remainders'] = len(result)
    result.append(encode(c))
    c = fixture()
    c['test'] = 1
    c['outputs'] = [(1, 5, 64, 0.5)]
    named['matching_simulation_consumes_rng_without_inventory_change'] = len(result)
    result.append(encode(c))
    for (requirement, heat, mode, item_dest, fluid_dest) in itertools.product(range(3), range(5), range(2), range(2), range(2)):
        c = fixture()
        c.update(required=requirement, heat=heat, mode=mode, item_dest=item_dest, fluid_dest=fluid_dest)
        c['outputs'] = [(1, 1, 64, 1)]
        c['fluid_outputs'] = [(1, 100)]
        result.append(encode(c))
    rng = random.Random(3125872144)
    for i in range(2500):
        c = fixture()
        c.update(test=rng.randrange(4) == 0, basin=rng.randrange(4) != 0, heat=rng.randrange(5), required=rng.randrange(3), mode=rng.randrange(3) == 0, item_dest=rng.randrange(2), fluid_dest=rng.randrange(2), items_present=rng.randrange(20) != 0, fluids_present=rng.randrange(20) != 0, valid=rng.randrange(20) != 0)
        c['test'] = int(c['test'])
        c['basin'] = int(c['basin'])
        c['mode'] = int(c['mode'])
        c['items_present'] = int(c['items_present'])
        c['fluids_present'] = int(c['fluids_present'])
        c['valid'] = int(c['valid'])
        c['ingredients'] = [rng.randrange(1, 32) for _ in range(rng.randrange(6))]
        for slot in range(18):
            if rng.randrange(4) == 0:
                key = rng.randrange(5)
                c['items'][slot] = (key, rng.choice([1, 2, 3, 15, 16, 60, 63, 64]), 64)
        if c['basin']:
            c['fluid_ingredients'] = [(rng.randrange(3), rng.choice([100, 200, 300, 500, 1000])) for _ in range(rng.randrange(3))]
            c['outputs'] = [(rng.randrange(5), rng.randrange(5), 64, rng.choice([0, 0.25, 0.5, 1])) for _ in range(rng.randrange(5))]
            c['fluid_outputs'] = [(rng.randrange(3), rng.choice([0, 100, 200, 300, 700, 1000, 1500, 2000])) for _ in range(rng.randrange(3))]
        else:
            c['outputs'] = [(rng.randrange(5), 1, 64, 1)]
        for tank in range(4):
            if rng.randrange(3) == 0:
                c['fluids'][tank] = (rng.randrange(3), rng.choice([100, 200, 300, 500, 700, 900, 1000]))
        result.append(encode(c))
    for (rpm, duration, ticks, processing, running, repeat) in itertools.product([0, 29, 30, 32, 64, 128, 256, 512, 1024, -32], [0, 1, 100, 250], [0, 10, 19, 20, 21, 39, 40], [-1, 0, 1, 61], [0, 1], [0, 1]):
        result.append('M ' + ' '.join(map(str, [rpm, 30, duration, running, ticks, processing, 160, -1, repeat, 0, 0])))
    for (cut, remove, start) in itertools.product([-1, 0, 1, 10, 19, 20, 21, 80, 100], range(2), range(2)):
        result.append('M ' + ' '.join(map(str, [32, 30, 0, 1, 10, 0, 180, cut, 0, start, remove])))
    for (name, case) in [('mixer_tick40_before_same_entity_saved_counter_update', 'M 32 30 0 1 0 0 40 -1 0 0 0'), ('mixer_same_entity_counter19_countdown_retained_before_completion', 'M 32 30 0 1 19 43 43 -1 0 0 0'), ('mixer_same_entity_counter19_countdown_retained_completion_tick84', 'M 32 30 0 1 19 43 44 -1 0 0 0')]:
        named[name] = len(result)
        result.append(case)
    return (result, named)
