"""Dependency-free SCIP index reader (standard library only).

python3 scripts/scip_decode.py stats   INDEX.scip
python3 scripts/scip_decode.py dump    INDEX.scip OUT.jsonl
python3 scripts/scip_decode.py refs    INDEX.scip QUERY [--limit N]
python3 scripts/scip_decode.py symbols INDEX.scip QUERY

QUERY is a dotted name such as Config.from_mapping; separators . :: # / are all accepted.
A SCIP symbol matches when its descriptor names end with the query parts.
Lines are 1-based. Roles: 1 Definition, 2 Import, 4 WriteAccess, 8 ReadAccess, 16 Generated, 32 Test, 64 ForwardDefinition.
"""
import collections
import json
import re
import sys


def varint(buf, i):
    shift = result = 0
    while True:
        b = buf[i]
        i += 1
        result |= (b & 0x7F) << shift
        if b < 0x80:
            return result, i
        shift += 7


def fields(buf):
    i, n = 0, len(buf)
    while i < n:
        key, i = varint(buf, i)
        num, wt = key >> 3, key & 7
        if wt == 0:
            v, i = varint(buf, i)
        elif wt == 2:
            ln, i = varint(buf, i)
            v = buf[i:i + ln]
            i += ln
        elif wt == 1:
            v = buf[i:i + 8]
            i += 8
        elif wt == 5:
            v = buf[i:i + 4]
            i += 4
        else:
            raise ValueError(f"wire type {wt}")
        yield num, wt, v


def packed(buf):
    out, i = [], 0
    while i < len(buf):
        v, i = varint(buf, i)
        out.append(v)
    return out


def parse_info(buf):
    info = {"symbol": "", "kind": 0, "display_name": "", "enclosing_symbol": "", "relationships": []}
    for n, _, v in fields(buf):
        if n == 1:
            info["symbol"] = v.decode()
        elif n == 5:
            info["kind"] = v
        elif n == 6:
            info["display_name"] = v.decode()
        elif n == 8:
            info["enclosing_symbol"] = v.decode()
        elif n == 4:
            rel = {"symbol": "", "is_reference": False, "is_implementation": False,
                   "is_type_definition": False, "is_definition": False}
            for rn, _, rv in fields(v):
                if rn == 1:
                    rel["symbol"] = rv.decode()
                elif rn == 2:
                    rel["is_reference"] = bool(rv)
                elif rn == 3:
                    rel["is_implementation"] = bool(rv)
                elif rn == 4:
                    rel["is_type_definition"] = bool(rv)
                elif rn == 5:
                    rel["is_definition"] = bool(rv)
            info["relationships"].append(rel)
    return info


def parse_occ(buf):
    rng, sym, roles, enc = [], "", 0, []
    for n, wt, v in fields(buf):
        if n == 1:
            rng = packed(v) if wt == 2 else rng + [v]
        elif n == 2:
            sym = v.decode()
        elif n == 3:
            roles = v
        elif n == 7:
            enc = packed(v) if wt == 2 else enc + [v]
    if len(rng) == 3:
        rng = [rng[0], rng[1], rng[0], rng[2]]
    if len(enc) == 3:
        enc = [enc[0], enc[1], enc[0], enc[2]]
    return {"start_line": rng[0] + 1, "start_col": rng[1], "end_line": rng[2] + 1, "end_col": rng[3],
            "symbol": sym, "roles": roles,
            "enc_start": enc[0] + 1 if enc else 0, "enc_end": enc[2] + 1 if enc else 0}


def load(path):
    with open(path, "rb") as handle:
        data = handle.read()
    meta = {}
    docs, external = [], []
    for num, _, v in fields(data):
        if num == 1:
            for mn, _, mv in fields(v):
                if mn == 2:
                    t = {"name": "", "version": "", "arguments": []}
                    for tn, _, tv in fields(mv):
                        if tn == 1:
                            t["name"] = tv.decode()
                        elif tn == 2:
                            t["version"] = tv.decode()
                        elif tn == 3:
                            t["arguments"].append(tv.decode())
                    meta["tool_info"] = t
                elif mn == 3:
                    meta["project_root"] = mv.decode()
                elif mn == 1:
                    meta["version"] = mv
        elif num == 2:
            doc = {"path": "", "language": "", "occurrences": [], "symbols": []}
            for dn, _, dv in fields(v):
                if dn == 1:
                    doc["path"] = dv.decode()
                elif dn == 2:
                    doc["occurrences"].append(parse_occ(dv))
                elif dn == 3:
                    doc["symbols"].append(parse_info(dv))
                elif dn == 4:
                    doc["language"] = dv.decode()
            docs.append(doc)
        elif num == 3:
            external.append(parse_info(v))
    return meta, docs, external


NAME_RE = re.compile(r"`((?:[^`]|``)*)`|([A-Za-z0-9_+\-$]+)")


def descriptors(symbol):
    """Return the list of (name, suffix) pairs of a non-local SCIP symbol."""
    if symbol.startswith("local "):
        return [(symbol[6:], "local")]
    parts = symbol.split(" ", 4)
    if len(parts) < 5:
        return []
    s, i, out = parts[4], 0, []
    while i < len(s):
        m = NAME_RE.match(s, i)
        if not m:
            if s[i] in "([":
                close = ")" if s[i] == "(" else "]"
                inner = NAME_RE.match(s, i + 1)
                if not inner or s[inner.end()] != close:
                    raise ValueError(f"cannot parse descriptor at {i} in {symbol!r}")
                name = inner.group(1).replace("``", "`") if inner.group(1) is not None else inner.group(2)
                out.append((name, "param" if close == ")" else "typeparam"))
                i = inner.end() + 1
                continue
            raise ValueError(f"cannot parse descriptor at {i} in {symbol!r}")
        name = m.group(1).replace("``", "`") if m.group(1) is not None else m.group(2)
        i = m.end()
        c = s[i] if i < len(s) else ""
        if c == "(":
            j = s.index(")", i)
            i = j + 1
            if i < len(s) and s[i] == ".":
                i += 1
            out.append((name, "method"))
        elif c in "/#.:!":
            out.append((name, {"/": "namespace", "#": "type", ".": "term", ":": "meta", "!": "macro"}[c]))
            i += 1
        else:
            out.append((name, "?"))
    return out


def query_parts(q):
    return [p for p in re.split(r"::|[.#/]", q) if p]


def matching_symbols(docs, external, q, loose=True):
    want = query_parts(q)
    loose_hits = []
    seen = {}
    for d in docs:
        for info in d["symbols"]:
            seen[info["symbol"]] = info
    for info in external:
        seen.setdefault(info["symbol"], info)
    for d in docs:
        for o in d["occurrences"]:
            seen.setdefault(o["symbol"], {"symbol": o["symbol"], "kind": 0, "display_name": ""})
    hits = []
    for sym, info in seen.items():
        if sym.startswith("local "):
            continue
        names = [n for n, _ in descriptors(sym)]
        if names[-len(want):] == want:
            hits.append(info)
        elif loose and names and names[-1] == want[-1] and is_subsequence(want[:-1], names[:-1]):
            loose_hits.append(info)
    return sorted(hits or loose_hits, key=lambda i: i["symbol"])


def is_subsequence(want, names):
    it = iter(names)
    return all(w in it for w in want)


def cmd_stats(path):
    meta, docs, external = load(path)
    roles = collections.Counter()
    ext = collections.Counter()
    langs = collections.Counter()
    symbols, local_syms, nonlocal_syms = set(), set(), set()
    refs_nonlocal = refs_local = defs = 0
    for d in docs:
        langs[d["language"]] += 1
        ext[d["path"].rsplit(".", 1)[-1] if "." in d["path"] else ""] += 1
        for o in d["occurrences"]:
            r = o["roles"]
            symbols.add(o["symbol"])
            is_local = o["symbol"].startswith("local ")
            (local_syms if is_local else nonlocal_syms).add(o["symbol"])
            for bit, name in ((1, "definition"), (2, "import"), (4, "write"), (8, "read"), (16, "generated"),
                              (32, "test"), (64, "forward_definition")):
                if r & bit:
                    roles[name] += 1
            if r & 1:
                defs += 1
            elif is_local:
                refs_local += 1
            else:
                refs_nonlocal += 1
    out = {
        "tool_info": meta.get("tool_info"), "project_root": meta.get("project_root"),
        "documents": len(docs), "documents_by_language": dict(langs), "documents_by_extension": dict(ext),
        "occurrences": sum(len(d["occurrences"]) for d in docs),
        "definition_occurrences": defs, "reference_occurrences": refs_local + refs_nonlocal,
        "reference_occurrences_nonlocal": refs_nonlocal, "reference_occurrences_local": refs_local,
        "role_counts": dict(roles), "distinct_symbols": len(symbols),
        "distinct_nonlocal_symbols": len(nonlocal_syms), "distinct_local_symbols": len(local_syms),
        "symbol_information_rows": sum(len(d["symbols"]) for d in docs), "external_symbols": len(external),
    }
    print(json.dumps(out, indent=1))


def cmd_dump(path, dst):
    _, docs, _ = load(path)
    with open(dst, "w") as out:
        for d in docs:
            info = {i["symbol"]: i for i in d["symbols"]}
            for o in d["occurrences"]:
                i = info.get(o["symbol"], {})
                out.write(json.dumps({"path": d["path"], "symbol": o["symbol"], "line": o["start_line"],
                                      "end_line": o["end_line"], "col": o["start_col"], "roles": o["roles"],
                                      "kind": i.get("kind", 0), "display_name": i.get("display_name", "")}) + "\n")


def cmd_symbols(path, q):
    _, docs, external = load(path)
    for i in matching_symbols(docs, external, q):
        print(i["symbol"], "| kind", i.get("kind", 0), "| display", i.get("display_name", ""))


def cmd_refs(path, q, limit):
    _, docs, external = load(path)
    hits = matching_symbols(docs, external, q)
    wanted = {h["symbol"] for h in hits}
    sites = collections.defaultdict(lambda: {"defs": [], "refs": []})
    for d in docs:
        for o in d["occurrences"]:
            if o["symbol"] in wanted:
                bucket = "defs" if o["roles"] & 1 else "refs"
                sites[o["symbol"]][bucket].append((d["path"], o["start_line"], o["roles"]))
    for h in hits:
        s = sites[h["symbol"]]
        refs = sorted(set(s["refs"]))
        ref_lines = sorted({(p, l) for p, l, _ in refs})
        print(f"SYMBOL {h['symbol']}")
        print(f"  kind={h.get('kind', 0)} display_name={h.get('display_name', '')!r} defs={len(s['defs'])} "
              f"ref_occurrences={len(s['refs'])} distinct_ref_lines={len(ref_lines)}")
        for p, l, r in sorted(set(s["defs"]))[:limit]:
            print(f"  DEF {p}:{l} roles={r}")
        for p, l, r in refs[:limit]:
            print(f"  REF {p}:{l} roles={r}")


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "stats":
        cmd_stats(sys.argv[2])
    elif cmd == "dump":
        cmd_dump(sys.argv[2], sys.argv[3])
    elif cmd == "symbols":
        cmd_symbols(sys.argv[2], sys.argv[3])
    elif cmd == "refs":
        limit = int(sys.argv[sys.argv.index("--limit") + 1]) if "--limit" in sys.argv else 50
        cmd_refs(sys.argv[2], sys.argv[3], limit)
    else:
        raise SystemExit(__doc__)
