"""Write a compact triage summary from Grype JSON without hiding scanner failures."""

from collections import Counter
from html import escape
import json
import os
from pathlib import Path
import sys


def cell(value):
    return escape(str(value)).replace("|", "&#124;").replace("\r", " ").replace("\n", " ")


def summarize(path):
    if not path.is_file():
        return f"## {cell(path.stem)}\n\nScan did not produce a report. Check the failed generation or scan step.\n"
    report = json.loads(path.read_text(encoding="utf-8"))
    matches = report["matches"]
    order = ["Critical", "High", "Medium", "Low", "Negligible", "Unknown"]
    counts = Counter(m["vulnerability"]["severity"] for m in matches)
    lines = [f"## {cell(path.stem)}", "", "Policy: High and Critical fail, including findings without a fix.", "",
             ", ".join(f"{s}: {counts[s]}" for s in order), "",
             "| Severity | Package | Installed | Fixed in | Advisory |",
             "|---|---|---|---|---|"]
    ranked = sorted(matches, key=lambda m: (order.index(m["vulnerability"]["severity"])
                    if m["vulnerability"]["severity"] in order else len(order),
                    m["artifact"]["name"], m["vulnerability"]["id"]))
    for match in ranked[:50]:
        vulnerability, artifact = match["vulnerability"], match["artifact"]
        fixes = vulnerability.get("fix", {}).get("versions", [])
        lines.append("| " + " | ".join(cell(v) for v in (
            vulnerability["severity"], artifact["name"], artifact["version"],
            ", ".join(fixes) or "Not available", vulnerability["id"])) + " |")
    lines.extend(["", f"Showing {min(50, len(matches))} of {len(matches)} matches. Full evidence is in the security artifact.", "",
                  "Lockfile inventories may include unused dependencies. Confirm affected runtime paths before deciding impact.", ""])
    return "\n".join(lines)


if __name__ == "__main__":
    summary = summarize(Path(sys.argv[1]))
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as output:
            output.write(summary)
    else:
        print(summary)
