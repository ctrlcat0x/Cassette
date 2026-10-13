"""Prepare verified playback binaries for the bundled-runtime Cargo feature."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import urllib.request
import zipfile


ROOT = Path(__file__).resolve().parents[1] / ".runtime" / "bundle"
SYSTEM = platform.system()
VERSIONS = {"yt-dlp/yt-dlp": "2026.08.19", "denoland/deno": "v2.9.7",
            "shinchiro/mpv-winbuild-cmake": "20261013"}
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
    return subprocess.check_output(args, text=True).strip()


def linux_mpv():
    source = Path(shutil.which("mpv") or "/usr/bin/mpv")
    shutil.copy2(source, ROOT / "mpv")
    libraries = ROOT / "lib"
    libraries.mkdir()
    pending = [source]
    copied = set()
    packages = {"mpv"}
    # Keep the host's glibc and dynamic loader together; bundle mpv's other dependencies.
    host = re.compile(r"^(?:ld-linux|libc\.|libm\.|libpthread\.|libdl\.|librt\.|libresolv\.|libutil\.)")
    while pending:
        current = pending.pop()
        for line in run("ldd", str(current)).splitlines():
            if "not found" in line:
                raise RuntimeError(line)
            match = re.search(r"=> (/\S+)", line)
            if not match:
                continue
            path = Path(match[1])
            if path.name in copied or host.match(path.name):
                continue
            copied.add(path.name)
            shutil.copy2(path, libraries / path.name)
            pending.append(path)
            owner = subprocess.run(["dpkg-query", "-S", str(path)], capture_output=True, text=True)
            if owner.returncode:
                owner = subprocess.run(["dpkg-query", "-S", str(path.resolve())], capture_output=True, text=True)
            packages.update(line.split(": ")[0].split(":")[0] for line in owner.stdout.splitlines() if ": " in line)
    for path in [ROOT / "mpv", *libraries.iterdir()]:
        subprocess.run(["patchelf", "--set-rpath", "$ORIGIN/lib" if path.parent == ROOT else "$ORIGIN", str(path)], check=True)
    for package in packages:
        copyright_file = Path("/usr/share/doc") / package / "copyright"
        if copyright_file.is_file():
            shutil.copy2(copyright_file, ROOT / "licenses" / (package + "-copyright"))
    MANIFEST["mpv"] = {"version": run("dpkg-query", "-W", "-f=${Version}", "mpv"),
                       "packages": sorted(packages), "source": "https://launchpad.net/ubuntu/+source/mpv",
                       "recipe": "Ubuntu 24.04 apt mpv; dependency closure relocated with patchelf"}


def macos_mpv():
    source = Path(run("brew", "--prefix", "mpv")) / "bin/mpv"
    libraries = ROOT / "lib"
    libraries.mkdir()
    pending = [source]
    targets = {source: ROOT / "mpv"}
    edges = {}
    while pending:
        current = pending.pop()
        shutil.copy2(current, targets[current])
        edges[current] = []
        for line in run("otool", "-L", str(current)).splitlines()[1:]:
            dep = line.strip().split(" (", 1)[0]
            if dep.startswith(("/usr/lib/", "/System/")):
                continue
            # Homebrew mpv and its dylibs carry absolute Cellar/opt install names.
            if not dep.startswith("/"):
                if dep == "@rpath/" + current.name:
                    continue
                raise RuntimeError(f"Unresolved Mach-O dependency {dep} in {current}")
            path = Path(dep)
            if path.resolve() == current.resolve():
                continue
            target = libraries / path.name
            if path not in targets:
                if target in targets.values():
                    other = next(key for key, value in targets.items() if value == target)
                    if other.resolve() != path.resolve():
                        raise RuntimeError(f"Dylib name collision: {path.name}")
                else:
                    targets[path] = target
                    pending.append(path)
            edges[current].append((dep, target))
    for original, path in targets.items():
        path.chmod(0o755)
        for dep, target in edges.get(original, []):
            relative = "@loader_path/" + ("lib/" if path.parent == ROOT else "") + target.name
            subprocess.run(["install_name_tool", "-change", dep, relative, str(path)], check=True)
        if path.parent == libraries:
            subprocess.run(["install_name_tool", "-id", "@rpath/" + path.name, str(path)], check=True)
        subprocess.run(["codesign", "--force", "--sign", "-", str(path)], check=True)
    # Preserve the licenses shipped with every Homebrew formula in the closure.
    cellar = Path(run("brew", "--cellar"))
    formulae = set()
    for original in targets:
        resolved = original.resolve()
        if cellar in resolved.parents:
            relative = resolved.relative_to(cellar)
            formulae.add(relative.parts[0])
            prefix = cellar / relative.parts[0] / relative.parts[1]
            for notice in prefix.rglob("*"):
                if notice.is_file() and notice.name.upper().startswith(("LICENSE", "COPYING", "COPYRIGHT")):
                    destination = ROOT / "licenses" / relative.parts[0] / notice.relative_to(prefix)
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(notice, destination)
    MANIFEST["mpv"] = {"version": run("brew", "list", "--versions", "mpv"),
                       "formulae": sorted(formulae), "source": "https://github.com/Homebrew/homebrew-core",
                       "recipe": "Homebrew mpv; recursive dylib closure relocated and ad-hoc signed"}


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
        if SYSTEM == "Windows":
            repo = "shinchiro/mpv-winbuild-cmake"
            with request(f"https://api.github.com/repos/{repo}/releases/tags/{VERSIONS[repo]}") as response:
                release = json.load(response)
            name = next(a["name"] for a in release["assets"] if re.fullmatch(r"mpv-x86_64-.*\.7z", a["name"]))
            archive = asset(repo, name, work)
            seven_zip = shutil.which("7z") or str(Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "7-Zip/7z.exe")
            subprocess.run([seven_zip, "e", str(archive), "mpv.exe", "*.dll", "-r", "-o" + str(ROOT), "-y"], check=True)
        elif SYSTEM == "Darwin":
            macos_mpv()
        else:
            linux_mpv()
        for repo, name, destination in [
            ("yt-dlp/yt-dlp", "LICENSE", "yt-dlp-LICENSE"),
            ("yt-dlp/yt-dlp", "THIRD_PARTY_LICENSES.txt", "yt-dlp-THIRD_PARTY_LICENSES.txt"),
            ("denoland/deno", "LICENSE.md", "deno-LICENSE.md"),
        ]:
            download(f"https://raw.githubusercontent.com/{repo}/{VERSIONS[repo]}/{name}", ROOT / "licenses" / destination)
        for name in ["Copyright", "LICENSE.GPL", "LICENSE.LGPL"]:
            download("https://raw.githubusercontent.com/mpv-player/mpv/master/" + name, ROOT / "licenses" / ("mpv-" + name))
    (ROOT / "versions.json").write_text(json.dumps(MANIFEST, indent=2) + "\n", encoding="utf-8")
    for name in ["mpv", "yt-dlp", "deno"]:
        path = ROOT / (name + ".exe" if SYSTEM == "Windows" else name)
        path.chmod(0o755)
        print(run(str(path), "--version").splitlines()[0])
    print(f"Prepared {sum(p.stat().st_size for p in ROOT.rglob('*') if p.is_file()) // 1048576} MiB runtime")


if __name__ == "__main__":
    main()
