"""Prepare verified playback binaries for the bundled-runtime Cargo feature."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import urllib.request
import zipfile


ROOT = Path(__file__).resolve().parents[1] / ".runtime" / "extractors"
SYSTEM = platform.system()
VERSIONS = {"yt-dlp/yt-dlp": "2026.08.19", "denoland/deno": "v2.9.7"}
MANIFEST = {}


def request(url):
    headers = {"User-Agent": "Cassette-runtime-build"}
    if url.startswith("https://api.github.com/") and os.environ.get("GH_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GH_TOKEN"]
    return urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=120)


def download(url, target):
    with request(url) as response, target.open("wb") as output:
        shutil.copyfileobj(response, output)


def asset(repo, filename, directory):
    tag = VERSIONS[repo]
    with request(f"https://api.github.com/repos/{repo}/releases/tags/{tag}") as response:
        release = json.load(response)
    item = next(a for a in release["assets"] if a["name"] == filename)
    digest = item.get("digest") or ""
    if not digest.startswith("sha256:"):
        raise RuntimeError(f"Upstream has no SHA-256 digest for {filename}")
    target = directory / filename
    download(item["browser_download_url"], target)
    actual = hashlib.file_digest(target.open("rb"), "sha256").hexdigest()
    if actual != digest.removeprefix("sha256:"):
        raise RuntimeError(f"SHA-256 mismatch: {filename}")
    MANIFEST[repo] = {"version": tag, "asset": filename, "sha256": actual,
                      "source": f"https://github.com/{repo}/tree/{tag}"}
    return target


def run(*args):
    return subprocess.check_output(args, text=True, encoding="utf-8", errors="replace").strip()


def main():
    if ROOT.exists():
        raise RuntimeError(f"Runtime staging already exists: {ROOT}. Use a clean checkout or remove that exact staging directory.")
    ROOT.mkdir(parents=True)
    (ROOT / "licenses").mkdir()
    with tempfile.TemporaryDirectory(prefix="cassette-runtime-") as temporary:
        work = Path(temporary)
        ytdlp_name = {"Windows": "yt-dlp.exe", "Darwin": "yt-dlp_macos", "Linux": "yt-dlp_linux"}[SYSTEM]
        shutil.copy2(asset("yt-dlp/yt-dlp", ytdlp_name, work), ROOT / ("yt-dlp.exe" if SYSTEM == "Windows" else "yt-dlp"))
        triple = {"Windows": "x86_64-pc-windows-msvc", "Darwin": "aarch64-apple-darwin", "Linux": "x86_64-unknown-linux-gnu"}[SYSTEM]
        deno = asset("denoland/deno", f"deno-{triple}.zip", work)
        executable = "deno.exe" if SYSTEM == "Windows" else "deno"
        with zipfile.ZipFile(deno) as archive, archive.open(executable) as source, (ROOT / executable).open("wb") as output:
            shutil.copyfileobj(source, output)
        for repo, name, destination in [
            ("yt-dlp/yt-dlp", "LICENSE", "yt-dlp-LICENSE"),
            ("yt-dlp/yt-dlp", "THIRD_PARTY_LICENSES.txt", "yt-dlp-THIRD_PARTY_LICENSES.txt"),
            ("denoland/deno", "LICENSE.md", "deno-LICENSE.md"),
        ]:
            download(f"https://raw.githubusercontent.com/{repo}/{VERSIONS[repo]}/{name}", ROOT / "licenses" / destination)
    for name, url in [
        ("libopus-COPYING", "https://raw.githubusercontent.com/xiph/opus/v1.6.1/COPYING"),
        ("Symphonia-MPL-2.0", "https://raw.githubusercontent.com/pdeljanov/Symphonia/v0.5.5/LICENSE"),
    ]:
        download(url, ROOT / "licenses" / name)
    (ROOT / "versions.json").write_text(json.dumps(MANIFEST, indent=2) + "\n", encoding="utf-8")
    for name in ["yt-dlp", "deno"]:
        path = ROOT / (name + ".exe" if SYSTEM == "Windows" else name)
        path.chmod(0o755)
        print(run(str(path), "--version").splitlines()[0])
    print(f"Prepared {sum(p.stat().st_size for p in ROOT.rglob('*') if p.is_file()) // 1048576} MiB runtime")


if __name__ == "__main__":
    main()
