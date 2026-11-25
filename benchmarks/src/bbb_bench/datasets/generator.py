"""Deterministic generator of scripted meetings with planted ground truth.

Each meeting is a list of speaker turns plus an annotation file listing the
tasks, decisions and budget figures that were planted in the dialogue. Audio is
produced from the script by a text-to-speech tool and is never committed.
"""

from __future__ import annotations

import argparse
import json
import random
from dataclasses import dataclass, field
from datetime import date, timedelta
from pathlib import Path
from typing import Any

from . import pools

WORDS_PER_MINUTE = 140
MEETING_DATE = date(2026, 11, 13)


@dataclass
class Meeting:
    lang: str
    meeting_date: date
    speakers: list[str]
    participants: list[str] | None = None
    turns: list[dict[str, Any]] = field(default_factory=list)
    tasks: list[dict[str, Any]] = field(default_factory=list)
    decisions: list[dict[str, Any]] = field(default_factory=list)
    budgets: list[dict[str, Any]] = field(default_factory=list)

    def say(self, speaker: str, text: str, lang: str | None = None) -> int:
        self.turns.append({"speaker": speaker, "lang": lang or self.lang, "text": text})
        return len(self.turns) - 1

    def words(self) -> int:
        return sum(len(t["text"].split()) for t in self.turns)


def _due_phrase(due: date, today: date, lang: str, rng: random.Random) -> str:
    """Spoken form of a due date: a weekday when within a week, else day and month."""
    delta = (due - today).days
    use_weekday = 1 <= delta <= 7 and rng.random() < 0.6
    if lang == "pl":
        if use_weekday:
            return pools.WEEKDAYS_PL[due.weekday()]
        return f"{pools.day_pl(due.day)} {pools.MONTHS_PL[due.month - 1]}"
    if use_weekday:
        return pools.WEEKDAYS_EN[due.weekday()]
    return f"{pools.MONTHS_EN[due.month - 1]} {pools.day_en(due.day)}"


def _random_due(today: date, rng: random.Random) -> date:
    due = today + timedelta(days=rng.randint(2, 21))
    while due.weekday() >= 5:  # weekdays only
        due += timedelta(days=1)
    return due


def _pick_action(lang: str, mixed: bool, rng: random.Random) -> tuple[str, list[str]]:
    """Return an action phrase in the turn language and the glossary terms it contains."""
    if lang == "pl":
        if mixed and rng.random() < 0.7:
            return rng.choice(pools.ACTIONS_MIXED)
        return rng.choice(pools.ACTIONS_PLAIN)[0], []
    return rng.choice(pools.ACTIONS_PLAIN)[1], []


def _others(speakers: list[str], *excluded: str) -> list[str]:
    return [s for s in speakers if s not in excluded]


def _external_person(m: Meeting, rng: random.Random) -> str:
    roster = set(m.participants if m.participants is not None else m.speakers)
    candidates = [p for p in pools.PEOPLE if p not in roster]
    return rng.choice(candidates) if candidates else "marek"


def _task_event(m: Meeting, rng: random.Random, mixed: bool) -> None:
    """A task is requested and accepted; variants: request, self, reassign, third_party, declined, unaccepted."""
    lang = m.lang
    # In mixed meetings a fifth of the turns are fully English.
    turn_lang = "en" if (mixed and rng.random() < 0.2) else lang
    a, b = rng.sample(m.speakers, 2)
    action, _terms = _pick_action(turn_lang, mixed, rng)
    due = _random_due(m.meeting_date, rng)
    due_s = _due_phrase(due, m.meeting_date, turn_lang, rng)
    variant = rng.choices(
        ["request", "self", "reassign", "third_party", "declined", "unaccepted"],
        [5, 2, 1, 1, 1, 1],
    )[0]
    pl = turn_lang == "pl"

    if variant == "declined":
        # Proposed and dropped: not a task.
        m.say(a, f"{b}, {'a może ty mógłbyś' if pl else 'maybe you could'} {action}?", turn_lang)
        m.say(
            b,
            "Nie, to nie ma sensu, odpuśćmy to."
            if pl
            else "No, that makes no sense, let us drop it.",
            turn_lang,
        )
        return

    if variant == "unaccepted":
        # Wish or tentative proposal without commitment: not a task.
        m.say(
            a,
            f"Fajnie byłoby kiedyś {action}." if pl else f"It would be nice to {action} sometime.",
            turn_lang,
        )
        m.say(
            b,
            "Zobaczymy, na razie nic nie obiecuję."
            if pl
            else "Let us see, I cannot promise anything right now.",
            turn_lang,
        )
        return

    if variant == "third_party":
        ext_id = _external_person(m, rng)
        ext_name = ext_id.capitalize()
        with_due = rng.random() < 0.5
        if pl:
            line = (
                f"{ext_name} przygotuje {action} na {due_s}."
                if with_due
                else f"{ext_name} ma do zrobienia: {action}."
            )
        else:
            line = (
                f"{ext_name} has to {action} by {due_s}."
                if with_due
                else f"{ext_name} has to {action}."
            )
        t = m.say(a, line, turn_lang)
        m.say(b, "Jasne, dopiszmy to." if pl else "Sure, let us note that down.", turn_lang)
        m.tasks.append(
            {
                "text": action,
                "assignee": None,
                "assignee_name": ext_name,
                "due": due.isoformat() if with_due else None,
                "turn": t,
                "quote": m.turns[t]["text"],
            }
        )
        return

    if variant == "self":
        t = m.say(
            a,
            f"Wezmę to na siebie: {action} do {due_s}."
            if pl
            else f"I will take this one: {action} by {due_s}.",
            turn_lang,
        )
        m.say(b, "Super, dziękuję." if pl else "Great, thank you.", turn_lang)
        assignee = a
    elif variant == "reassign":
        c = rng.choice(_others(m.speakers, a, b))
        t = m.say(
            a,
            f"{b}, czy możesz {action} do {due_s}?" if pl else f"{b}, can you {action} by {due_s}?",
            turn_lang,
        )
        m.say(
            b,
            f"Mam za dużo na głowie, niech to zrobi {c}."
            if pl
            else f"I have too much on my plate, let {c} do it.",
            turn_lang,
        )
        m.say(
            c,
            f"Dobrze, ja to zrobię do {due_s}." if pl else f"Fine, I will do it by {due_s}.",
            turn_lang,
        )
        assignee = c
    else:
        t = m.say(
            a,
            f"{b}, czy możesz {action} do {due_s}?" if pl else f"{b}, can you {action} by {due_s}?",
            turn_lang,
        )
        m.say(
            b,
            f"Tak, zrobię to do {due_s}." if pl else f"Sure, I will have it done by {due_s}.",
            turn_lang,
        )
        assignee = b

    m.tasks.append(
        {
            "text": action,
            "assignee": assignee,
            "due": due.isoformat(),
            "turn": t,
            "quote": m.turns[t]["text"],
        }
    )


def _decision_event(m: Meeting, rng: random.Random, mixed: bool) -> None:
    lang = "en" if (mixed and rng.random() < 0.2) else m.lang
    a, b = rng.sample(m.speakers, 2)
    if lang == "pl":
        pool = pools.DECISIONS_MIXED if mixed and rng.random() < 0.6 else pools.DECISIONS_PL
        text = rng.choice(pool)
        t = m.say(a, f"Ustalmy, że {text}.", lang)
        m.say(b, "Zgoda.", lang)
    else:
        text = rng.choice(pools.DECISIONS_EN)
        t = m.say(a, f"Let us agree that {text}.", lang)
        m.say(b, "Agreed.", lang)
    m.decisions.append({"text": text, "turn": t, "quote": m.turns[t]["text"]})


def _budget_event(m: Meeting, rng: random.Random, mixed: bool) -> None:
    lang = "en" if (mixed and rng.random() < 0.2) else m.lang
    a, b = rng.sample(m.speakers, 2)
    pl_amount, en_amount, value = rng.choice(pools.AMOUNTS)
    pl_item, en_item = rng.choice(pools.BUDGET_ITEMS)
    if lang == "pl":
        t = m.say(a, f"Budżet na {pl_item} to {pl_amount} złotych.", lang)
        m.say(b, "Przyjąłem, wpisuję do planu.", lang)
        item, currency = pl_item, "PLN"
    else:
        t = m.say(a, f"The budget for {en_item} is {en_amount} euros.", lang)
        m.say(b, "Got it, adding it to the plan.", lang)
        item, currency = en_item, "EUR"
    m.budgets.append(
        {
            "item": item,
            "amount": value,
            "currency": currency,
            "turn": t,
            "quote": m.turns[t]["text"],
        }
    )


def _filler_event(m: Meeting, rng: random.Random, mixed: bool) -> None:
    a, b = rng.sample(m.speakers, 2)
    if m.lang == "en":
        pool = pools.FILLER_EN
    elif mixed:
        pool = pools.FILLER_MIXED + pools.FILLER_PL
    else:
        pool = pools.FILLER_PL
    m.say(a, rng.choice(pool), "pl" if pool is not pools.FILLER_EN else "en")
    m.say(b, rng.choice(pool), "pl" if pool is not pools.FILLER_EN else "en")


def generate_meeting(
    lang: str,
    n_speakers: int,
    minutes: float,
    seed: int,
    mixed: bool = False,
    meeting_date: date = MEETING_DATE,
    people: list[str] | None = None,
    task_density: str = "dense",
) -> Meeting:
    """Generate a scripted meeting.

    `lang` is the dominant language ("pl" or "en"). With `mixed`, Polish
    sentences contain English technical terms and some turns are fully English.
    `people` fixes the participants (their ids); otherwise they are sampled.
    `task_density` is "dense" (many commitments, stress test) or "sparse" (realistic).
    """
    if lang not in ("pl", "en"):
        raise ValueError("lang must be 'pl' or 'en'")
    if people is not None:
        unknown = [p for p in people if p not in pools.PEOPLE]
        if unknown or len(people) != n_speakers:
            raise ValueError("people must be known ids and match n_speakers")
    if not 2 <= n_speakers <= len(pools.PEOPLE):
        raise ValueError(f"n_speakers must be between 2 and {len(pools.PEOPLE)}")
    rng = random.Random(seed)
    speakers = list(people) if people is not None else rng.sample(pools.PEOPLE, n_speakers)
    m = Meeting(lang=lang, meeting_date=meeting_date, speakers=speakers)
    target_words = int(minutes * WORDS_PER_MINUTE)
    events = [_task_event, _decision_event, _budget_event, _filler_event]
    # events: task, decision, budget, filler. `sparse` gives about 15-25 tasks per 15 minutes.
    weights = [2, 1, 1, 10] if task_density == "sparse" else [5, 2, 1, 4]
    opener = speakers[0]
    m.say(
        opener,
        "Dzień dobry wszystkim, zaczynamy spotkanie."
        if lang == "pl"
        else "Good morning everyone, let us begin.",
    )
    while m.words() < target_words:
        rng.choices(events, weights)[0](m, rng, mixed)
    m.say(
        opener,
        "Dziękuję wszystkim, to na dziś wszystko."
        if lang == "pl"
        else "Thank you all, that is all for today.",
    )
    return m


def to_files(m: Meeting, out: Path, glossary: list[str] | None = None) -> None:
    """Write script.json, annotation.json, reference.txt and transcript.txt."""
    out.mkdir(parents=True, exist_ok=True)
    script = {"lang": m.lang, "speakers": m.speakers, "turns": m.turns}
    full_text = " ".join(t["text"] for t in m.turns)
    terms = []
    for term in glossary or []:
        terms.extend([term] * full_text.lower().count(term.lower()))
    annotation = {
        "meeting_date": m.meeting_date.isoformat(),
        "lang": m.lang,
        "speakers": m.speakers,
        "participants": m.participants if m.participants is not None else m.speakers,
        "tasks": m.tasks,
        "decisions": m.decisions,
        "budgets": m.budgets,
        "terms": terms,
    }
    (out / "script.json").write_text(json.dumps(script, ensure_ascii=False, indent=1), "utf-8")
    (out / "annotation.json").write_text(
        json.dumps(annotation, ensure_ascii=False, indent=1), "utf-8"
    )
    (out / "reference.txt").write_text(full_text + "\n", "utf-8")
    (out / "transcript.txt").write_text(
        "\n".join(f"{t['speaker']}: {t['text']}" for t in m.turns) + "\n", "utf-8"
    )


def main() -> None:
    p = argparse.ArgumentParser(description="Generate a scripted meeting.")
    p.add_argument("--lang", choices=["pl", "en"], required=True)
    p.add_argument("--mixed", action="store_true", help="Polish with English terms")
    p.add_argument("--speakers", type=int, required=True)
    p.add_argument("--minutes", type=float, required=True)
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--people", default=None, help="Comma-separated participant ids")
    p.add_argument("--out", type=Path, required=True)
    args = p.parse_args()
    people = args.people.split(",") if args.people else None
    m = generate_meeting(
        args.lang, args.speakers, args.minutes, args.seed, args.mixed, people=people
    )
    to_files(m, args.out, pools.GLOSSARY if args.mixed else None)
    print(f"{args.out}: {len(m.turns)} turns, {m.words()} words, {len(m.tasks)} tasks")


if __name__ == "__main__":
    main()
