"""Synthetic meetings for measuring status proposals (`status_changes`) of the extraction step.

Twelve short fictional meetings, six in English and six in Polish. Each one comes with open
issues (and sometimes an open topic) from earlier meetings, and its transcript says clearly what
happened to each of them. The label of a target is one of:

- `done`: a person says the work is finished (or the group agrees to close the topic). This is
  the only label for which a proposal is expected.
- `blocked`, `reopened`, `in_progress`, `untouched`: the target is mentioned or not, but the work
  is not finished. A proposal for any of these is a false positive.

Two meetings (`en-s4`, `pl-s4`) have no finished target at all. They also contain finished work
that is on no list (a newsletter), which must not become a proposal either. In `en-s6` and
`pl-s6` the issue 74 / 134 appears only as the issue of a previous item (not in the list of open
issues), which `reduce` accepts as a target too.

All names, projects and numbers are made up. Text only, nothing here comes from a real meeting.
"""

from __future__ import annotations

from typing import Any

DONE = "done"
NOT_DONE_KINDS = ("blocked", "reopened", "in_progress", "untouched")


def _issue(number: int, title: str, assignee: str) -> dict[str, str]:
    return {"id": f"#{number}", "title": title, "assignee": assignee}


def _previous(
    number: int | None, text: str, assignee: str, due: str, meeting: str
) -> dict[str, Any]:
    out: dict[str, Any] = {
        "meeting": meeting,
        "type": "task",
        "text": text,
        "assignee": assignee,
        "due": due,
    }
    if number is not None:
        out["issue"] = f"#{number}"
    return out


def _scenario(
    sid: str,
    lang: str,
    meeting_date: str,
    speakers: list[str],
    issues: list[dict[str, str]],
    previous: list[dict[str, Any]],
    turns: list[tuple[str, str]],
    expect: dict[str, str],
    evidence: dict[str, int],
    topics: list[dict[str, str]] | None = None,
) -> dict[str, Any]:
    return {
        "id": sid,
        "lang": lang,
        "meeting_date": meeting_date,
        "speakers": speakers,
        "open_issues": issues,
        "open_topics": topics or [],
        "previous_items": previous,
        "turns": [{"speaker": s, "lang": lang, "text": t} for s, t in turns],
        "expect": expect,
        "evidence": evidence,
    }


SCENARIOS: list[dict[str, Any]] = [
    # English ---------------------------------------------------------------------------------
    _scenario(
        "en-s1",
        "en",
        "2026-11-16",
        ["dana", "ethan", "priya", "luke"],
        [
            _issue(21, "Set up the staging database", "ethan"),
            _issue(22, "Write the onboarding email sequence", "priya"),
            _issue(23, "Fix the login timeout on Android", "luke"),
            _issue(24, "Order the demo tablets", "dana"),
        ],
        [
            _previous(21, "Set up the staging database", "ethan", "2026-11-13", "2026-11-09"),
            _previous(
                22, "Write the onboarding email sequence", "priya", "2026-11-20", "2026-11-09"
            ),
            _previous(23, "Fix the login timeout on Android", "luke", "2026-11-18", "2026-11-09"),
            _previous(24, "Order the demo tablets", "dana", "2026-11-17", "2026-11-09"),
        ],
        [
            ("dana", "Morning all, short sync today."),
            (
                "ethan",
                "The staging database is up. I finished the migration over the weekend and the "
                "smoke tests pass, so issue 21 is done.",
            ),
            ("dana", "Great, we can close that one."),
            (
                "priya",
                "The onboarding emails are about half written. I need two more days for the "
                "last three emails.",
            ),
            (
                "luke",
                "For the Android login timeout, I could reproduce it but I have not found the "
                "cause yet.",
            ),
            ("dana", "Okay. And the demo tablets, I am still waiting for a quote from the vendor."),
            ("luke", "Should we talk about the dashboard colors?"),
            ("dana", "Not today, let us leave that for the design review."),
            ("ethan", "I will write the backup script for staging by Friday."),
            ("dana", "Good, that is all for now."),
        ],
        {"#21": DONE, "#22": "in_progress", "#23": "in_progress", "#24": "in_progress"},
        {"#21": 1},
    ),
    _scenario(
        "en-s2",
        "en",
        "2026-11-17",
        ["omar", "nina", "felix", "grace"],
        [
            _issue(31, "Add CSV export to the reports page", "omar"),
            _issue(32, "Update the privacy policy text", "grace"),
            _issue(33, "Migrate the build to the new CI runner", "felix"),
            _issue(34, "Review the pricing page copy", "nina"),
        ],
        [
            _previous(31, "Add CSV export to the reports page", "omar", "2026-11-13", "2026-11-10"),
            _previous(32, "Update the privacy policy text", "grace", "2026-11-18", "2026-11-10"),
            _previous(
                33, "Migrate the build to the new CI runner", "felix", "2026-11-19", "2026-11-10"
            ),
            _previous(34, "Review the pricing page copy", "nina", "2026-11-17", "2026-11-10"),
        ],
        [
            (
                "omar",
                "The CSV export is merged and shipped to production since Friday, nothing is "
                "left to do there.",
            ),
            (
                "grace",
                "The privacy policy update is blocked, legal has not sent the final wording.",
            ),
            (
                "felix",
                "The CI migration is blocked as well, we are waiting for the new runner licence.",
            ),
            (
                "nina",
                "I reviewed the pricing page copy and sent my comments, but the author still "
                "has to apply them.",
            ),
            ("grace", "So that one is not finished yet."),
            ("nina", "Right, not finished."),
            ("omar", "Next week I will start on the invoice template."),
        ],
        {"#31": DONE, "#32": "blocked", "#33": "blocked", "#34": "in_progress"},
        {"#31": 0},
    ),
    _scenario(
        "en-s3",
        "en",
        "2026-11-18",
        ["ivan", "hannah", "tom", "zoe"],
        [
            _issue(41, "Fix the duplicate notifications bug", "tom"),
            _issue(42, "Publish the API documentation", "hannah"),
            _issue(43, "Create the support FAQ", "zoe"),
        ],
        [
            _previous(41, "Fix the duplicate notifications bug", "tom", "2026-11-13", "2026-11-11"),
            _previous(42, "Publish the API documentation", "hannah", "2026-11-17", "2026-11-11"),
            _previous(43, "Create the support FAQ", "zoe", "2026-11-20", "2026-11-11"),
        ],
        [
            (
                "ivan",
                "Tom, the duplicate notifications are back. Users reported it again this morning.",
            ),
            (
                "tom",
                "Yes, I thought it was fixed last week, but the fix did not cover the retry "
                "path. It needs more work.",
            ),
            ("ivan", "So issue 41 stays open."),
            (
                "hannah",
                "The API documentation is published, the site is live and the links in the "
                "readme point to it. I am calling issue 42 resolved.",
            ),
            ("ivan", "Great."),
            ("zoe", "The support FAQ, I have not started, I was on holiday."),
            ("ivan", "No problem, please start this week."),
        ],
        {"#41": "reopened", "#42": DONE, "#43": "untouched"},
        {"#42": 3},
    ),
    _scenario(
        "en-s4",
        "en",
        "2026-11-19",
        ["carl", "rita", "sam"],
        [
            _issue(51, "Prepare the investor update", "rita"),
            _issue(52, "Replace the old printer", "carl"),
            _issue(53, "Test the new signup flow", "sam"),
        ],
        [
            _previous(51, "Prepare the investor update", "rita", "2026-11-20", "2026-11-12"),
            _previous(52, "Replace the old printer", "carl", "2026-11-25", "2026-11-12"),
            _previous(53, "Test the new signup flow", "sam", "2026-11-19", "2026-11-12"),
        ],
        [
            ("carl", "We should replace the printer soon, the old one jammed twice this week."),
            (
                "rita",
                "I have a draft of the investor update, but the numbers for October are missing.",
            ),
            ("sam", "I started testing the signup flow, three of the ten test cases are done."),
            ("carl", "Last week we finished the newsletter and it went out on Thursday."),
            ("rita", "Yes, that went well. Can we talk about the conference?"),
            ("sam", "The conference is in March, we only need to book the hotel."),
            ("carl", "Let us decide the budget next time."),
            ("rita", "I will send the investor draft to Carl by Thursday."),
        ],
        {"#51": "in_progress", "#52": "untouched", "#53": "in_progress"},
        {},
    ),
    _scenario(
        "en-s5",
        "en",
        "2026-11-20",
        ["john", "mia", "raj", "lena"],
        [
            _issue(61, "Renew the domain name", "mia"),
            _issue(62, "Fix the typo on the pricing page", "raj"),
            _issue(63, "Prepare the quarterly budget sheet", "lena"),
        ],
        [
            _previous(61, "Renew the domain name", "mia", "2026-11-19", "2026-11-13"),
            _previous(62, "Fix the typo on the pricing page", "raj", "2026-11-19", "2026-11-13"),
            _previous(63, "Prepare the quarterly budget sheet", "lena", "2026-11-20", "2026-11-13"),
        ],
        [
            ("john", "Mia, the domain renewal, is that finished?"),
            ("mia", "Yes, I renewed it for another three years yesterday and I have the receipt."),
            ("john", "Good. Raj, the typo on the pricing page?"),
            ("raj", "Fixed and deployed this morning."),
            ("john", "And the quarterly budget sheet, Lena?"),
            ("lena", "Not yet. I am waiting for the numbers from finance."),
            (
                "john",
                "Now the office plant supplier. We compared the three offers and we agree to "
                "take the second one, so we can close that topic.",
            ),
            ("mia", "Agreed."),
            ("lena", "Agreed."),
        ],
        {"#61": DONE, "#62": DONE, "#63": "blocked", "t4": DONE},
        {"#61": 1, "#62": 3, "t4": 6},
        topics=[{"id": "t4", "text": "Decide on the office plant supplier"}],
    ),
    _scenario(
        "en-s6",
        "en",
        "2026-11-23",
        ["paula", "max", "ines", "victor"],
        [
            _issue(71, "Add dark mode to the settings screen", "max"),
            _issue(72, "Clean up the shared drive", "ines"),
            _issue(73, "Prepare the workshop slides", "victor"),
        ],
        [
            _previous(
                71, "Add dark mode to the settings screen", "max", "2026-11-27", "2026-11-16"
            ),
            _previous(72, "Clean up the shared drive", "ines", "2026-11-23", "2026-11-16"),
            _previous(73, "Prepare the workshop slides", "victor", "2026-11-25", "2026-11-16"),
            _previous(74, "Set up the monitoring alerts", "paula", "2026-11-20", "2026-11-16"),
        ],
        [
            (
                "paula",
                "Let us go through the open issues. Issue 74, the monitoring alerts. I finished "
                "that on Tuesday, the alerts fire correctly in the test environment.",
            ),
            ("max", "Issue 71, dark mode. I have the design, I will start coding tomorrow."),
            (
                "ines",
                "Issue 72, the shared drive. I deleted about half of the old folders, the rest "
                "needs a decision from Victor.",
            ),
            (
                "victor",
                "I will look at it tomorrow. And the workshop slides are with the reviewer, "
                "they should come back on Wednesday.",
            ),
            ("paula", "Then nothing else on my side."),
        ],
        {"#74": DONE, "#71": "untouched", "#72": "in_progress", "#73": "in_progress"},
        {"#74": 0},
    ),
    # Polish ----------------------------------------------------------------------------------
    _scenario(
        "pl-s1",
        "pl",
        "2026-11-16",
        ["ola", "tomek", "kasia", "marek"],
        [
            _issue(81, "Skonfigurować środowisko testowe", "tomek"),
            _issue(82, "Napisać instrukcję dla nowych użytkowników", "kasia"),
            _issue(83, "Naprawić błąd w eksporcie do PDF", "marek"),
            _issue(84, "Zamówić naklejki z logo", "ola"),
        ],
        [
            _previous(81, "Skonfigurować środowisko testowe", "tomek", "2026-11-13", "2026-11-09"),
            _previous(
                82,
                "Napisać instrukcję dla nowych użytkowników",
                "kasia",
                "2026-11-20",
                "2026-11-09",
            ),
            _previous(83, "Naprawić błąd w eksporcie do PDF", "marek", "2026-11-18", "2026-11-09"),
            _previous(84, "Zamówić naklejki z logo", "ola", "2026-11-17", "2026-11-09"),
        ],
        [
            ("ola", "Dzień dobry, zaczynamy krótkie spotkanie."),
            (
                "tomek",
                "Środowisko testowe jest gotowe, skończyłem konfigurację w piątek i wszystkie "
                "testy przechodzą, więc zgłoszenie 81 mogę zamknąć.",
            ),
            ("ola", "Świetnie, zamykamy."),
            (
                "kasia",
                "Instrukcja dla nowych użytkowników jest napisana w połowie, potrzebuję jeszcze "
                "dwóch dni.",
            ),
            (
                "marek",
                "Z błędem w eksporcie do PDF mam problem, nie umiem go odtworzyć na swoim "
                "komputerze.",
            ),
            ("ola", "Naklejki z logo jeszcze nie zamówione, czekam na wycenę od drukarni."),
            ("tomek", "Zrobię kopię zapasową bazy testowej do czwartku."),
            ("ola", "Dobrze, to wszystko na dziś."),
        ],
        {"#81": DONE, "#82": "in_progress", "#83": "in_progress", "#84": "in_progress"},
        {"#81": 1},
    ),
    _scenario(
        "pl-s2",
        "pl",
        "2026-11-17",
        ["zosia", "bartek", "ewa", "adam"],
        [
            _issue(91, "Dodać filtr dat na liście zamówień", "bartek"),
            _issue(92, "Zaktualizować politykę prywatności", "zosia"),
            _issue(93, "Przenieść build na nowy serwer CI", "adam"),
            _issue(94, "Przejrzeć tekst strony z cennikiem", "ewa"),
        ],
        [
            _previous(
                91, "Dodać filtr dat na liście zamówień", "bartek", "2026-11-13", "2026-11-10"
            ),
            _previous(
                92, "Zaktualizować politykę prywatności", "zosia", "2026-11-18", "2026-11-10"
            ),
            _previous(93, "Przenieść build na nowy serwer CI", "adam", "2026-11-19", "2026-11-10"),
            _previous(94, "Przejrzeć tekst strony z cennikiem", "ewa", "2026-11-17", "2026-11-10"),
        ],
        [
            (
                "bartek",
                "Filtr dat na liście zamówień jest wdrożony na produkcję od piątku, nic tu już "
                "nie zostało do zrobienia.",
            ),
            (
                "zosia",
                "Polityka prywatności stoi w miejscu, dział prawny nie przysłał końcowego "
                "brzmienia.",
            ),
            (
                "adam",
                "Przeniesienie buildu też jest zablokowane, czekamy na licencję na nowy serwer.",
            ),
            (
                "ewa",
                "Tekst cennika przejrzałam i wysłałam uwagi, ale autor jeszcze ich nie wprowadził.",
            ),
            ("zosia", "Czyli to jeszcze nie jest skończone."),
            ("ewa", "Tak, jeszcze nie."),
            ("bartek", "W przyszłym tygodniu zaczynam szablon faktury."),
        ],
        {"#91": DONE, "#92": "blocked", "#93": "blocked", "#94": "in_progress"},
        {"#91": 0},
    ),
    _scenario(
        "pl-s3",
        "pl",
        "2026-11-18",
        ["iza", "janek", "maja", "olek"],
        [
            _issue(101, "Naprawić zdublowane powiadomienia", "janek"),
            _issue(102, "Opublikować dokumentację API", "maja"),
            _issue(103, "Przygotować FAQ dla wsparcia", "olek"),
        ],
        [
            _previous(
                101, "Naprawić zdublowane powiadomienia", "janek", "2026-11-13", "2026-11-11"
            ),
            _previous(102, "Opublikować dokumentację API", "maja", "2026-11-17", "2026-11-11"),
            _previous(103, "Przygotować FAQ dla wsparcia", "olek", "2026-11-20", "2026-11-11"),
        ],
        [
            (
                "iza",
                "Janek, zdublowane powiadomienia wróciły. Użytkownicy zgłosili to znowu dziś rano.",
            ),
            (
                "janek",
                "Tak, myślałem, że poprawka z zeszłego tygodnia wystarczy, ale nie objęła "
                "ponawiania wysyłki. Trzeba to zrobić od nowa.",
            ),
            ("iza", "Czyli zgłoszenie 101 zostaje otwarte."),
            (
                "maja",
                "Dokumentacja API jest opublikowana, strona działa, a odnośniki w pliku readme "
                "prowadzą już na nią. Uznaję zgłoszenie 102 za rozwiązane.",
            ),
            ("iza", "Super."),
            ("olek", "FAQ dla wsparcia jeszcze nie ruszyłem, byłem na urlopie."),
            ("iza", "Nie szkodzi, zacznij w tym tygodniu."),
        ],
        {"#101": "reopened", "#102": DONE, "#103": "untouched"},
        {"#102": 3},
    ),
    _scenario(
        "pl-s4",
        "pl",
        "2026-11-19",
        ["renata", "stefan", "kuba"],
        [
            _issue(111, "Przygotować raport dla inwestorów", "renata"),
            _issue(112, "Wymienić starą drukarkę", "stefan"),
            _issue(113, "Przetestować nową rejestrację", "kuba"),
        ],
        [
            _previous(
                111, "Przygotować raport dla inwestorów", "renata", "2026-11-20", "2026-11-12"
            ),
            _previous(112, "Wymienić starą drukarkę", "stefan", "2026-11-25", "2026-11-12"),
            _previous(113, "Przetestować nową rejestrację", "kuba", "2026-11-19", "2026-11-12"),
        ],
        [
            ("stefan", "Trzeba wymienić drukarkę, ta stara zacięła się w tym tygodniu dwa razy."),
            ("renata", "Mam szkic raportu dla inwestorów, ale brakuje liczb za październik."),
            (
                "kuba",
                "Zacząłem testować rejestrację, trzy z dziesięciu przypadków testowych mam za "
                "sobą.",
            ),
            ("stefan", "W zeszłym tygodniu skończyliśmy newsletter i poszedł w czwartek."),
            ("renata", "Tak, wyszedł dobrze. Porozmawiajmy o konferencji."),
            ("kuba", "Konferencja jest w marcu, zostało tylko zarezerwować hotel."),
            ("stefan", "Budżet ustalmy na następnym spotkaniu."),
            ("renata", "Wyślę szkic raportu do Stefana do czwartku."),
        ],
        {"#111": "in_progress", "#112": "untouched", "#113": "in_progress"},
        {},
    ),
    _scenario(
        "pl-s5",
        "pl",
        "2026-11-20",
        ["jan", "magda", "rafal", "lena"],
        [
            _issue(121, "Przedłużyć domenę firmową", "magda"),
            _issue(122, "Poprawić literówkę na stronie z cennikiem", "rafal"),
            _issue(123, "Przygotować arkusz budżetu kwartalnego", "lena"),
        ],
        [
            _previous(121, "Przedłużyć domenę firmową", "magda", "2026-11-19", "2026-11-13"),
            _previous(
                122,
                "Poprawić literówkę na stronie z cennikiem",
                "rafal",
                "2026-11-19",
                "2026-11-13",
            ),
            _previous(
                123, "Przygotować arkusz budżetu kwartalnego", "lena", "2026-11-20", "2026-11-13"
            ),
        ],
        [
            ("jan", "Magda, przedłużenie domeny, zrobione?"),
            (
                "magda",
                "Tak, przedłużyłam ją wczoraj na kolejne trzy lata i mam potwierdzenie.",
            ),
            ("jan", "Dobrze. Rafał, literówka na stronie z cennikiem?"),
            ("rafal", "Poprawiona i wdrożona dziś rano."),
            ("jan", "A arkusz budżetu kwartalnego, Leno?"),
            ("lena", "Jeszcze nie. Czekam na liczby z działu finansów."),
            (
                "jan",
                "To teraz dostawca roślin do biura. Porównaliśmy trzy oferty i zgadzamy się "
                "wziąć drugą, więc ten temat możemy zamknąć.",
            ),
            ("magda", "Zgoda."),
            ("lena", "Zgoda."),
        ],
        {"#121": DONE, "#122": DONE, "#123": "blocked", "t4": DONE},
        {"#121": 1, "#122": 3, "t4": 6},
        topics=[{"id": "t4", "text": "Wybór dostawcy roślin do biura"}],
    ),
    _scenario(
        "pl-s6",
        "pl",
        "2026-11-23",
        ["daria", "maks", "iga", "wiktor"],
        [
            _issue(131, "Dodać tryb ciemny w ustawieniach", "maks"),
            _issue(132, "Posprzątać wspólny dysk", "iga"),
            _issue(133, "Przygotować slajdy na warsztaty", "wiktor"),
        ],
        [
            _previous(131, "Dodać tryb ciemny w ustawieniach", "maks", "2026-11-27", "2026-11-16"),
            _previous(132, "Posprzątać wspólny dysk", "iga", "2026-11-23", "2026-11-16"),
            _previous(133, "Przygotować slajdy na warsztaty", "wiktor", "2026-11-25", "2026-11-16"),
            _previous(134, "Ustawić alerty monitoringu", "daria", "2026-11-20", "2026-11-16"),
        ],
        [
            (
                "daria",
                "Przejdźmy po otwartych zgłoszeniach. Zgłoszenie 134, alerty monitoringu. "
                "Skończyłam je we wtorek, alerty odpalają się poprawnie w środowisku testowym.",
            ),
            ("maks", "Zgłoszenie 131, tryb ciemny. Mam projekt, jutro zaczynam kodować."),
            (
                "iga",
                "Zgłoszenie 132, wspólny dysk. Usunęłam około połowy starych folderów, o reszcie "
                "musi zdecydować Wiktor.",
            ),
            (
                "wiktor",
                "Spojrzę na to jutro. A slajdy na warsztaty są u recenzenta, wrócą w środę.",
            ),
            ("daria", "To po mojej stronie nic więcej."),
        ],
        {"#134": DONE, "#131": "untouched", "#132": "in_progress", "#133": "in_progress"},
        {"#134": 0},
    ),
]


def expected_done(scenario: dict[str, Any]) -> set[str]:
    """Targets for which a proposal is expected."""
    return {t for t, kind in scenario["expect"].items() if kind == DONE}
