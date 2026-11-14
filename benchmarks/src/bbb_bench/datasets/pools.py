"""Word pools for the scripted meeting generator. All content is fictional."""

from __future__ import annotations

# Fictional participants. The id doubles as the spoken name.
PEOPLE = [
    "alex", "kasia", "jan", "marta", "piotr", "ola", "tomek", "ewa", "bartek", "zosia",
    "marek", "ania", "pawel", "iza", "darek", "magda", "lukasz", "basia", "michal", "nina",
]  # fmt: skip

# Glossary used by the mixed-language set (English terms inside Polish speech).
GLOSSARY = ["CAN bus", "GNSS", "firmware", "LiDAR", "telemetry", "bootloader", "payload", "torque"]

MONTHS_PL = [
    "stycznia", "lutego", "marca", "kwietnia", "maja", "czerwca", "lipca", "sierpnia",
    "września", "października", "listopada", "grudnia",
]  # fmt: skip
MONTHS_EN = [
    "January", "February", "March", "April", "May", "June", "July", "August",
    "September", "October", "November", "December",
]  # fmt: skip
WEEKDAYS_PL = ["poniedziałku", "wtorku", "środy", "czwartku", "piątku", "soboty", "niedzieli"]
WEEKDAYS_EN = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]

_PL_UNITS = [
    "", "pierwszego", "drugiego", "trzeciego", "czwartego", "piątego", "szóstego", "siódmego",
    "ósmego", "dziewiątego",
]  # fmt: skip
_PL_TEENS = [
    "dziesiątego", "jedenastego", "dwunastego", "trzynastego", "czternastego", "piętnastego",
    "szesnastego", "siedemnastego", "osiemnastego", "dziewiętnastego",
]  # fmt: skip
_EN_UNITS = [
    "", "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth",
]  # fmt: skip
_EN_TEENS = [
    "tenth", "eleventh", "twelfth", "thirteenth", "fourteenth", "fifteenth", "sixteenth",
    "seventeenth", "eighteenth", "nineteenth",
]  # fmt: skip


def day_pl(day: int) -> str:
    """Genitive ordinal for a day of month, e.g. 21 -> 'dwudziestego pierwszego'."""
    if day < 10:
        return _PL_UNITS[day]
    if day < 20:
        return _PL_TEENS[day - 10]
    tens = {2: "dwudziestego", 3: "trzydziestego"}[day // 10]
    return tens if day % 10 == 0 else f"{tens} {_PL_UNITS[day % 10]}"


def day_en(day: int) -> str:
    """Ordinal for a day of month, e.g. 21 -> 'twenty first'."""
    if day < 10:
        return _EN_UNITS[day]
    if day < 20:
        return _EN_TEENS[day - 10]
    tens = {2: "twenty", 3: "thirty"}[day // 10]
    if day % 10 == 0:
        return {2: "twentieth", 3: "thirtieth"}[day // 10]
    return f"{tens} {_EN_UNITS[day % 10]}"


# Task actions: (Polish infinitive phrase, English phrase, mixed phrase, glossary terms inside).
ACTIONS_PLAIN = [
    ("przygotować plan testów", "prepare the test plan"),
    ("zamówić nowe czujniki", "order the new sensors"),
    ("napisać instrukcję montażu", "write the assembly guide"),
    ("sprawdzić wycenę dostawcy", "check the supplier quote"),
    ("zarezerwować salę na warsztat", "book the workshop room"),
    ("zrobić makietę obudowy", "build the enclosure mockup"),
    ("wysłać harmonogram prac do klienta", "send the work schedule to the client"),
    ("zaktualizować listę zakupów", "update the shopping list"),
    ("przygotować prezentację dla zarządu", "prepare the board presentation"),
    ("zebrać uwagi od zespołu", "collect feedback from the team"),
]
ACTIONS_MIXED = [
    ("przygotować specyfikację CAN bus", ["CAN bus"]),
    ("zamówić moduł GNSS", ["GNSS"]),
    ("napisać nowy firmware sterownika", ["firmware"]),
    ("przeprowadzić kalibrację LiDAR", ["LiDAR"]),
    ("wysłać raport z telemetry do zespołu", ["telemetry"]),
    ("skompilować bootloader dla płytki", ["bootloader"]),
    ("zmierzyć masę payload", ["payload"]),
    ("sprawdzić torque silnika", ["torque"]),
]

DECISIONS_PL = [
    "zostajemy przy obecnym dostawcy",
    "przesuwamy wdrożenie na następny miesiąc",
    "rezygnujemy z drugiej wersji obudowy",
    "wybieramy mniejszy wariant płytki",
    "spotykamy się w każdy piątek rano",
]
DECISIONS_EN = [
    "we stay with the current supplier",
    "we move the rollout to next month",
    "we drop the second enclosure version",
    "we pick the smaller board variant",
    "we meet every Friday morning",
]
DECISIONS_MIXED = [
    "zostajemy przy obecnym CAN bus",
    "przesuwamy test GNSS na następny miesiąc",
    "wybieramy nowszy firmware",
    "rezygnujemy z drugiego modułu LiDAR",
]

# Amounts: spoken form in each language and numeric value in PLN or EUR.
AMOUNTS = [
    ("pięć tysięcy", "five thousand", 5000),
    ("osiem tysięcy", "eight thousand", 8000),
    ("dwanaście tysięcy", "twelve thousand", 12000),
    ("dwadzieścia tysięcy", "twenty thousand", 20000),
    ("trzydzieści tysięcy", "thirty thousand", 30000),
    ("sto pięćdziesiąt tysięcy", "one hundred fifty thousand", 150000),
]
BUDGET_ITEMS = [
    ("zakup czujników", "sensor purchase"),
    ("druk trójwymiarowy", "three-d printing"),
    ("licencje oprogramowania", "software licenses"),
    ("wyjazd na targi", "the trade fair trip"),
    ("narzędzia warsztatowe", "workshop tools"),
]

FILLER_PL = [
    "Dobrze, notuję.",
    "Rozumiem, to ma sens.",
    "Wczoraj testowaliśmy to na stanowisku i wyniki były stabilne.",
    "Muszę to jeszcze sprawdzić z zespołem, ale wygląda obiecująco.",
    "Przejdźmy do kolejnego punktu.",
    "Czy ktoś ma pytania do tego tematu?",
    "Dostawca obiecał odpowiedź na początku tygodnia.",
    "To powinno być prostsze niż poprzednim razem.",
    "Zgadzam się, ale zostawmy margines na opóźnienia.",
    "Dzięki, to bardzo pomaga.",
]
FILLER_EN = [
    "Okay, noted.",
    "I see, that makes sense.",
    "We tested this on the bench yesterday and the results were stable.",
    "I still need to check with the team, but it looks promising.",
    "Let us move on to the next item.",
    "Does anyone have questions about this topic?",
    "The supplier promised an answer early next week.",
    "This should be simpler than last time.",
    "I agree, but let us leave some margin for delays.",
    "Thanks, that really helps.",
]
FILLER_MIXED = [
    "Wczoraj puściliśmy telemetry z całego przejazdu i wygląda stabilnie.",
    "Firmware jest już wgrany na dwie płytki, trzecia czeka na bootloader.",
    "Kalibracja LiDAR zajęła więcej czasu, niż zakładaliśmy.",
    "Sygnał GNSS był słaby w hali, ale na zewnątrz działał dobrze.",
    "Na CAN bus widzę czasem zgubione ramki.",
    "Payload jest cięższy, niż było w specyfikacji.",
]
