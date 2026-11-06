#!/usr/bin/python3
"""The two halves of run/without-launchers.sh: what Lobby says about every game in one view of the
disk (collect), and what got worse between the launchers being there and gone (compare)."""
import json
import os
import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

LOOKED_AT = ("Game", "Download", "StoreApp", "WrongSystem")
# What of a game has to survive, as it's stored in the library.
KEPT = ("protection", "launch_options", "frame_rate", "favourite", "played", "proton")
ENTRY_PATHS = ("folder", "program", "prefix")


def paths_in(words):
    for word in words:
        word = word.split("=", 1)[1] if re.match(r"^[A-Z_]+=", word) else word
        if word.startswith("/"):
            yield word


def missing(paths):
    return sorted({p for p in paths if p and p.startswith("/") and not os.path.exists(p)})


def run(command):
    p = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, text=True, errors="replace", timeout=120)
    return p.returncode, p.stdout + p.stderr


def look(lobby, game):
    id = game["id"]
    _, how = run([lobby, "how", id])
    code, plan = run([lobby, "how", id, "--json"])
    command, cwd = [], None
    if code == 0:
        try:
            parsed = json.loads(plan)
            command, cwd = parsed.get("command") or [], parsed.get("cwd")
        except ValueError:
            pass
    saves = sorted(line for line in how.splitlines() if re.match(r"^[^:\n]{1,60}: /", line) and not line.startswith(("Warning", "Move", "Uninstall")))
    return id, {"how": how, "starts": code == 0, "why_not": "" if code == 0 else plan.strip(), "command": command, "gone_from_command": missing(list(paths_in(command)) + [cwd]), "saves": saves, "gone_saves": missing(s.split(": ", 1)[1] for s in saves)}


def collect(lobby, out):
    code, scan = run([lobby, "scan", "--json"])
    with open(os.path.join(out, "scan.txt"), "w") as f:
        f.write(scan)
    if code != 0:
        print(f"The scan failed: {scan[-400:]}")
    data = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    with open(os.path.join(data, "lobby/library.json")) as f:
        games = json.load(f)["games"]
    with open(os.path.join(data, "lobby/playtime.json")) as f:
        playtime = json.load(f)
    chosen = [g for g in games if g["kind"] in LOOKED_AT]
    if os.environ.get("ONLY"):
        chosen = [g for g in chosen if g["id"] == os.environ["ONLY"]]
    with ThreadPoolExecutor(8) as pool:
        looks = dict(pool.map(lambda g: look(lobby, g), chosen))
    pictures = {g["id"]: missing([g.get("kept_cover"), g.get("kept_icon")]) for g in games}
    files = {g["id"]: missing(e.get(k) for e in g["entries"] for k in ENTRY_PATHS) for g in games}
    with open(os.path.join(out, "result.json"), "w") as f:
        json.dump({"games": games, "playtime": playtime, "looks": looks, "pictures": pictures, "files": files}, f)
    print(f"{out}: {len(games)} games, {len(looks)} looked at")


def played(record, id):
    p = record["playtime"].get(id, {})
    return (p.get("own", 0), p.get("launchers", 0), p.get("last", 0))


def compare(a_dir, b_dir):
    with open(os.path.join(a_dir, "result.json")) as f:
        a = json.load(f)
    with open(os.path.join(b_dir, "result.json")) as f:
        b = json.load(f)
    after = {g["id"]: g for g in b["games"]}
    worse = {}

    def bad(game, what):
        worse.setdefault(game, []).append(what)

    for g in a["games"]:
        id, name = g["id"], g["name"]
        n = after.get(id)
        if n is None:
            if g["kind"] != "Owned":
                bad(name, "left the library")
            continue
        if n["kind"] != g["kind"]:
            bad(name, f"was {g['kind']}, now {n['kind']}")
        if (n.get("missing") and not g.get("missing")) or (n.get("gone") and not g.get("gone")):
            bad(name, "now marked missing")
        for what in b["pictures"].get(id, []):
            if what not in a["pictures"].get(id, []):
                bad(name, f"picture gone: {what}")
        for k in ("kept_cover", "kept_icon"):
            if g.get(k) and not n.get(k):
                bad(name, f"{k} forgotten")
        for what in b["files"].get(id, []):
            if what not in a["files"].get(id, []):
                bad(name, f"file gone: {what}")
        for k in KEPT:
            if g.get(k) != n.get(k):
                bad(name, f"{k} changed: {g.get(k)!r} -> {n.get(k)!r}")
        if played(b, id) < played(a, id):
            bad(name, f"play time fell: {played(a, id)} -> {played(b, id)}")
        was, now = a["looks"].get(id), b["looks"].get(id)
        if not was or not now:
            continue
        if was["starts"] and not now["starts"]:
            bad(name, f"can't start now: {now['why_not'][:200]}")
        for what in now["gone_from_command"]:
            if what not in was["gone_from_command"]:
                bad(name, f"its start needs what's gone: {what}")
        for what in was["saves"]:
            if what not in now["saves"]:
                bad(name, f"saves lost: {what}")
        if was["command"] and now["command"] and was["command"] != now["command"]:
            bad(name, f"starts differently: {' '.join(was['command'])[:220]} -> {' '.join(now['command'])[:220]}")

    print(f"{len(a['games'])} games, {len(a['looks'])} installed ones looked at closely; {len(b['games'])} games with the launchers gone.")
    print(f"Worse with the launchers gone: {len(worse)}")
    for name in sorted(worse):
        print(f"\n{name}")
        for what in worse[name]:
            print(f"  {what}")
    broken = {id: w for id, w in a["looks"].items() if w["gone_from_command"] or w["gone_saves"]}
    print(f"\nAlready broken with the launchers there (not their fault): {len(broken)}")
    for id, w in sorted(broken.items()):
        print(f"  {id}: {', '.join(w['gone_from_command'] + w['gone_saves'])[:200]}")


if __name__ == "__main__":
    if sys.argv[1] == "collect":
        collect(sys.argv[2], sys.argv[3])
    else:
        compare(sys.argv[2], sys.argv[3])
