"""Repeat the publication-count-controlled campaign using production configuration."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

bench = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(bench / "range-writeback"))
import campaign

def main():
    p = argparse.ArgumentParser()
    p.add_argument("raw", type=Path)
    p.add_argument("report", type=Path)
    p.add_argument("--image", default="catdb-prod-range8-final")
    args = p.parse_args()
    args.raw.mkdir(parents=True, exist_ok=False)
    args.report.mkdir(parents=True, exist_ok=False)
    campaign.IMAGE = args.image
    ids = {name: subprocess.check_output(["docker","image","inspect","--format","{{.Id}}",name],
                                         text=True).strip()
           for name in (args.image, "catdb-fifo-products")}
    (args.report/"images.json").write_text(json.dumps(ids,indent=2), encoding="utf8")
    pilot = campaign.campaign(args.raw/"pilot", args.report, True)
    print("Pilot dispersion check:", campaign.passes(pilot), flush=True)
    campaign.campaign(args.raw/"full", args.report, False)
    for script, params in [
        ("verify_range.py", [args.report, "--raw", args.raw/"full"]),
        ("exposure.py", [args.raw/"full", args.report]),
    ]:
        subprocess.run([sys.executable, "-B", bench/"range-writeback"/script, *params],check=True)
    subprocess.run([sys.executable, "-B", Path(__file__).with_name("report.py"),args.report],check=True)

if __name__=="__main__":
    main()
