"""Generate evals/scenarios/long.json: ~110 turns, probes far from the facts they test.

Usage: uv run python -m evals.gen_long
"""

import json
import random
from pathlib import Path
from typing import Any

# (fact, probe question, expect groups)
FACTS = [
    ("I'm Noor, a freelance architect based in Rotterdam.", "Where am I based?", [["rotterdam"]]),
    (
        "My studio is called Kade Studio and it's in the Katendrecht district.",
        "What's my studio called?",
        [["kade"]],
    ),
    (
        "My partner Elif is a pediatric nurse at Erasmus MC.",
        "Where does Elif work?",
        [["erasmus"]],
    ),
    ("We have a cat named Miso who is deaf.", "What's special about our cat Miso?", [["deaf"]]),
    (
        "My biggest client right now is the housing corporation Woonstad.",
        "Who is my biggest client?",
        [["woonstad"]],
    ),
    (
        "I cycle to the studio; it takes 25 minutes.",
        "How do I get to the studio?",
        [["cycl", "bike"]],
    ),
    ("I'm allergic to shellfish.", "Is there any food I must avoid?", [["shellfish"]]),
    (
        "My accountant is Bram; taxes are due at the end of April.",
        "Who does my taxes?",
        [["bram"]],
    ),
    (
        "I play in a five-a-side football team on Thursday nights.",
        "Which night is my football?",
        [["thursday"]],
    ),
    (
        "My mother lives in Ankara and I visit twice a year.",
        "Where does my mother live?",
        [["ankara"]],
    ),
    (
        "I use Rhino and Revit for modelling, never SketchUp.",
        "Which modelling tools do I use?",
        [["rhino"], ["revit"]],
    ),
    (
        "The Woonstad project is a 120-unit timber housing block called Hout Hof.",
        "What is the Woonstad project?",
        [["hout hof", "timber"]],
    ),
    (
        "My studio assistant is Daan; he's part-time, three days a week.",
        "Who is my assistant?",
        [["daan"]],
    ),
    (
        "I'm training for the Rotterdam half marathon in October.",
        "What race am I training for?",
        [["half marathon", "half-marathon"]],
    ),
    (
        "My car is an old red Volvo that I only use for site visits.",
        "What car do I drive?",
        [["volvo"]],
    ),
    (
        "The Hout Hof planning permission hearing is on 14 September.",
        "When is the Hout Hof hearing?",
        [["september"]],
    ),
    (
        "I speak Dutch, Turkish and English; my French is terrible.",
        "Which languages do I speak?",
        [["dutch"], ["turkish"]],
    ),
    ("I bill Woonstad at 95 euros an hour.", "What's my hourly rate for Woonstad?", [["95"]]),
    (
        "Elif and I are getting married next June in Izmir.",
        "Where and when is the wedding?",
        [["izmir"], ["june"]],
    ),
    (
        "My favourite building is the Van Nelle factory.",
        "What's my favourite building?",
        [["van nelle"]],
    ),
    (
        "I have a second, smaller client: a bakery called Brood & Zo that wants a new shopfront.",
        "What does Brood & Zo want from me?",
        [["shopfront", "storefront", "front"]],
    ),
    (
        "I take Fridays off to sketch; no client calls on Fridays.",
        "Which day do I keep free of client calls?",
        [["friday"]],
    ),
    (
        "My laptop is a ThinkPad and I refuse to use a Mac.",
        "What laptop do I use?",
        [["thinkpad"]],
    ),
    (
        "Daan is studying for his architecture master's at TU Delft.",
        "Where is Daan studying?",
        [["delft"]],
    ),
    (
        "I bank with Bunq for the business account.",
        "Which bank is my business account with?",
        [["bunq"]],
    ),
]

# (fact index, update text, contradicts, asserts, probe, expect, reject)
UPDATES = [
    (
        5,
        "Update: I moved the studio to the Delfshaven district, so the cycle is now 40 minutes.",
        ["25"],
        ["40"],
        "How long is my cycle to the studio now?",
        [["40"]],
        ["25"],
    ),
    (
        17,
        "Woonstad agreed to a rate increase: I now bill them 110 euros an hour.",
        ["95"],
        ["110"],
        "What's my current hourly rate for Woonstad?",
        [["110"]],
        ["95"],
    ),
    (
        15,
        "The Hout Hof hearing got postponed to 2 October.",
        ["september"],
        ["october"],
        "When is the Hout Hof hearing now?",
        [["october"]],
        ["september"],
    ),
    (
        12,
        "Daan is now full-time, five days a week.",
        ["three"],
        ["five"],
        "How many days a week does Daan work now?",
        [["five", "5"]],
        ["three"],
    ),
    (
        8,
        "Football moved to Monday nights.",
        ["thursday"],
        ["monday"],
        "Which night is football now?",
        [["monday"]],
        ["thursday"],
    ),
]

GOALS = [
    "Help me keep a list of things to do before the Hout Hof hearing.",
    "Remind me to send Bram the Q2 invoices before the end of the month.",
]

RECAPS = [
    ("Summarise what you know about my work in a few sentences.", [["woonstad"], ["hout hof"]]),
    ("Summarise what you know about my personal life.", [["elif"], ["rotterdam"]]),
    ("Who are the people you know about in my life and what do they do?", [["elif"], ["daan"]]),
    ("What are my open to-dos?", [["hearing", "hout hof"], ["invoice", "bram"]]),
    ("Give me a one-paragraph profile of me.", [["architect"], ["rotterdam"]]),
]

DISTRACTORS = [
    "What's the difference between cement and concrete?",
    "Any good podcast about urban planning?",
    "How do I get red wine out of a wool rug?",
    "Is it going to rain in Rotterdam this weekend, do you think?",
    "What's the tallest building in the Netherlands?",
    "Explain cross-laminated timber in two sentences.",
    "How many calories are in a stroopwafel?",
    "Who designed the Erasmus Bridge?",
    "What's a good stretch for tight hamstrings?",
    "Translate 'planning permission' into Dutch.",
    "Why do my ears pop on the train through a tunnel?",
    "Recommend a sci-fi novel.",
    "What's the etymology of the word 'architect'?",
    "How long should I boil an egg for a soft yolk?",
    "Is Brutalism making a comeback?",
    "What's the capital of Kazakhstan these days?",
    "Give me a mnemonic for the planets.",
    "What's the best way to clean a chain on a bike?",
    "How do passive houses stay warm?",
    "Any tips for a stiff neck from drawing all day?",
    "What year did the Van Nelle factory become a UNESCO site?",
    "Is it 'less' or 'fewer' with countable nouns?",
    "What's a good warm-up before a run?",
    "How does a heat pump work, briefly?",
    "What's the Dutch word for a cat?",
    "How do I convert square feet to square metres?",
    "Which is heavier, a litre of water or a litre of oil?",
    "Explain what an acoustic rating for a wall means.",
    "What's the plural of 'moose'?",
    "Any idea why my sourdough is so dense?",
    "What's the difference between a facade and an elevation?",
    "Who painted The Night Watch?",
    "How many time zones does Turkey have?",
    "What does BIM stand for?",
    "Is it cheaper to fly or take the train from Rotterdam to Paris?",
    "Give me a random fun fact about bridges.",
    "What's a decent beginner's chess opening?",
    "How do I stop a door from squeaking?",
    "What's the boiling point of water at altitude?",
    "How long do LED bulbs typically last?",
    "What's the difference between a loggia and a balcony?",
    "Any easy houseplants that survive neglect?",
    "How do you pronounce 'Scheveningen'?",
    "What's the origin of the Dutch gable?",
    "How much sleep do adults actually need?",
]


def build() -> dict[str, Any]:
    rng = random.Random(0)
    turns: list[dict[str, Any]] = []
    fact_pos: dict[int, int] = {}
    update_pos: dict[int, int] = {}
    fillers = iter(DISTRACTORS)

    def add(text: str, tag: str, **kw: Any) -> int:
        turns.append({"text": text, "tag": tag, **kw})
        return len(turns) - 1

    def filler() -> None:
        add(next(fillers), "distractor")

    # Block 1: plant facts, one filler after every second fact, goals at 10 and 30.
    for i, (fact, _, _) in enumerate(FACTS):
        fact_pos[i] = add(fact, "fact")
        if i % 2 == 1:
            filler()
        if len(turns) in (10, 30):
            add(GOALS[0 if len(turns) == 10 else 1], "goal")

    # Block 2: updates, each followed by a probe of an early fact and a filler.
    early = [i for i in range(len(FACTS)) if i not in {u[0] for u in UPDATES}][:5]
    for k, (_, text, contra, asserts, *_rest) in enumerate(UPDATES):
        update_pos[k] = add(text, "update", contradicts=contra, asserts=asserts)
        f = early[k]
        add(FACTS[f][1], "probe", expect=FACTS[f][2], refs=[fact_pos[f]])
        filler()

    # Block 3: remaining fact probes, update probes and recaps, shuffled, fillers between.
    updated = {u[0] for u in UPDATES}
    rest: list[tuple[str, dict[str, Any]]] = [
        (FACTS[f][1], {"expect": FACTS[f][2], "refs": [fact_pos[f]]})
        for f in range(len(FACTS))
        if f not in early and f not in updated
    ]
    rest += [
        (u[4], {"expect": u[5], "reject": u[6], "refs": [update_pos[k]]})
        for k, u in enumerate(UPDATES)
    ]
    rest += [(q, {"expect": e, "refs": []}) for q, e in RECAPS]
    rng.shuffle(rest)
    for n, (q, kw) in enumerate(rest):
        add(q, "probe", **kw)
        if n % 2 == 0:
            filler()
    return {"cycle_every": 5, "turns": turns}


if __name__ == "__main__":
    data = build()
    out = Path(__file__).resolve().parent / "scenarios" / "long.json"
    out.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n")
    probes = sum(t["tag"] == "probe" for t in data["turns"])
    print(f"wrote {out}: {len(data['turns'])} turns, {probes} probes")
