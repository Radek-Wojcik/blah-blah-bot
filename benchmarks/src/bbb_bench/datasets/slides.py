"""Generate the `slides` set: synthetic slides, charts and diagrams with known key facts.

    uv run --group vision python -m bbb_bench.datasets.slides --out data/slides

Every image is drawn from fictional data. `facts.json` lists, per image, the key
facts (names and numbers) that a description must contain.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

W, H = 1280, 720
INK, MUTED, ACCENT, BG = (24, 28, 36), (96, 104, 120), (30, 100, 200), (250, 250, 252)


def font(size: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype("arialbd.ttf" if bold else "arial.ttf", size)


def canvas(title: str) -> tuple[Image.Image, ImageDraw.ImageDraw]:
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    d.text((60, 40), title, font=font(46, True), fill=INK)
    d.line((60, 105, W - 60, 105), fill=ACCENT, width=4)
    return img, d


def bullets(title: str, items: list[str]) -> Image.Image:
    img, d = canvas(title)
    for i, item in enumerate(items):
        y = 160 + i * 95
        d.ellipse((70, y + 12, 86, y + 28), fill=ACCENT)
        d.text((110, y), item, font=font(36), fill=INK)
    return img


def bar_chart(title: str, unit: str, data: list[tuple[str, int]]) -> Image.Image:
    img, d = canvas(title)
    left, bottom, top = 140, 620, 190
    d.line((left, top, left, bottom), fill=MUTED, width=3)
    d.line((left, bottom, W - 80, bottom), fill=MUTED, width=3)
    d.text((60, 125), unit, font=font(28), fill=MUTED)
    peak = max(v for _, v in data)
    slot = (W - 80 - left) // len(data)
    for i, (label, value) in enumerate(data):
        x0 = left + i * slot + slot // 6
        x1 = left + (i + 1) * slot - slot // 6
        y = bottom - int((bottom - top - 40) * value / peak)
        d.rectangle((x0, y, x1, bottom), fill=ACCENT)
        d.text((x0, y - 38), str(value), font=font(30, True), fill=INK)
        d.text((x0, bottom + 12), label, font=font(28), fill=INK)
    return img


def table(title: str, header: list[str], rows: list[list[str]]) -> Image.Image:
    img, d = canvas(title)
    col = (W - 120) // len(header)
    for r, row in enumerate([header, *rows]):
        y = 150 + r * 80
        if r == 0:
            d.rectangle((60, y, W - 60, y + 70), fill=(225, 232, 245))
        for c, cell in enumerate(row):
            d.text((80 + c * col, y + 16), cell, font=font(32, r == 0), fill=INK)
        d.line((60, y + 72, W - 60, y + 72), fill=MUTED, width=1)
    return img


def diagram(title: str, nodes: list[str], edges: list[tuple[int, int, str]]) -> Image.Image:
    img, d = canvas(title)
    xs = [140 + i * (W - 280) // (len(nodes) - 1) for i in range(len(nodes))]
    y = 330
    for x, name in zip(xs, nodes, strict=True):
        d.rounded_rectangle((x - 105, y, x + 105, y + 110), radius=14, outline=ACCENT, width=4)
        tw = d.textlength(name, font=font(30, True))
        d.text((x - tw / 2, y + 36), name, font=font(30, True), fill=INK)
    for a, b, label in edges:
        x0, x1 = xs[a] + 105, xs[b] - 105
        d.line((x0, y + 55, x1, y + 55), fill=INK, width=4)
        d.polygon([(x1, y + 55), (x1 - 18, y + 43), (x1 - 18, y + 67)], fill=INK)
        tw = d.textlength(label, font=font(26))
        d.text(((x0 + x1) / 2 - tw / 2, y + 12), label, font=font(26), fill=MUTED)
    return img


SLIDES = [
    ("bullets-1", lambda: bullets("Rover test status", ["Chassis frame built: 12 of 14 parts", "Motor torque: 48 Nm measured", "Battery pack: 24 V, 20 Ah", "Field trial planned for 3 December"]),
     ["12", "14", "48 Nm", "24 V", "20 Ah", "3 December"]),
    ("bullets-2", lambda: bullets("Budget decisions", ["Sensors: 8000 EUR approved", "3D printing: 5000 EUR approved", "Trade fair trip postponed to March", "Supplier contract renewed for 2 years"]),
     ["8000", "5000", "March", "2 years"]),
    ("bullets-3", lambda: bullets("Open risks", ["GNSS signal weak indoors", "LiDAR calibration takes 6 hours", "Firmware v2.4 not yet released", "Two team members on leave in week 47"]),
     ["GNSS", "6 hours", "v2.4", "47"]),
    ("bars-1", lambda: bar_chart("Tasks closed per week", "tasks", [("W44", 14), ("W45", 22), ("W46", 9), ("W47", 31)]),
     ["W44", "14", "W45", "22", "W46", "9", "W47", "31"]),
    ("bars-2", lambda: bar_chart("Monthly cost", "thousand EUR", [("Aug", 48), ("Sep", 52), ("Oct", 61), ("Nov", 57), ("Dec", 66)]),
     ["Aug", "48", "Sep", "52", "Oct", "61", "Nov", "57", "Dec", "66"]),
    ("bars-3", lambda: bar_chart("Test runs by outcome", "runs", [("Pass", 87), ("Fail", 13), ("Skipped", 5)]),
     ["Pass", "87", "Fail", "13", "Skipped", "5"]),
    ("table-1", lambda: table("Supplier comparison", ["Supplier", "Price", "Delivery"], [["Northwind", "4200", "10 days"], ["Contoso", "3900", "21 days"], ["Fabrikam", "4500", "7 days"]]),
     ["Northwind", "4200", "10 days", "Contoso", "3900", "21 days", "Fabrikam", "4500", "7 days"]),
    ("table-2", lambda: table("Owner assignments", ["Task", "Owner", "Due"], [["Mount design", "Alex", "20 Nov"], ["GNSS order", "Kasia", "25 Nov"], ["Test plan", "Jan", "01 Dec"]]),
     ["Mount design", "Alex", "20 Nov", "GNSS order", "Kasia", "25 Nov", "Test plan", "Jan", "01 Dec"]),
    ("table-3", lambda: table("Sensor specification", ["Sensor", "Range", "Rate"], [["LiDAR", "100 m", "20 Hz"], ["GNSS", "global", "10 Hz"], ["IMU", "n/a", "200 Hz"]]),
     ["LiDAR", "100 m", "20 Hz", "GNSS", "10 Hz", "IMU", "200 Hz"]),
    ("diagram-1", lambda: diagram("Data path", ["Sensor", "CAN bus", "Controller", "Telemetry"], [(0, 1, "raw"), (1, 2, "frames"), (2, 3, "5 Hz")]),
     ["Sensor", "CAN bus", "Controller", "Telemetry", "5 Hz"]),
    ("diagram-2", lambda: diagram("Release pipeline", ["Commit", "Build", "Test", "Deploy"], [(0, 1, "push"), (1, 2, "artifact"), (2, 3, "approved")]),
     ["Commit", "Build", "Test", "Deploy"]),
    ("diagram-3", lambda: diagram("Power system", ["Battery", "Converter", "Motor"], [(0, 1, "24 V"), (1, 2, "12 V")]),
     ["Battery", "Converter", "Motor", "24 V", "12 V"]),
]  # fmt: skip


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--out", type=Path, default=Path("data/slides"))
    args = p.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    facts = {}
    for name, make, key_facts in SLIDES:
        make().save(args.out / f"{name}.png")
        facts[f"{name}.png"] = key_facts
    (args.out / "facts.json").write_text(json.dumps(facts, indent=1), encoding="utf-8")
    print(f"{len(SLIDES)} slides written to {args.out}")


if __name__ == "__main__":
    main()
