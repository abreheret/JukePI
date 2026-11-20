#!/usr/bin/env python3
"""Copies finished torrent downloads that contain MP3 or FLAC files into the music library, filed as
<Artist>/<Album>/ from their tags, then asks the music server to rescan. The music server plays MP3 only:
FLAC files are converted to MP3 (MP3_BITRATE, 320k by default), tags and embedded cover kept.

Runs on the host (systemd units next to this file), as the user that owns the library.
Needs ffmpeg and ffprobe.

qBittorrent records each finished torrent in FINISHED_LIST (see torrent-finished.sh); paths there are the
ones qBittorrent sees (CONTAINER_DOWNLOADS, e.g. /downloads/...), mapped here to DOWNLOADS.

- The MP3/FLAC files of a torrent are grouped by album tag: one torrent can hold several albums (discography).
  An album that comes both as MP3 and FLAC in the same torrent keeps its MP3 files only.
- Artist folder: the album artist tag, else the artist of more than half of the tracks, else "Various Artists".
  Album folder: the album tag. Without tags, both come from the folder name ("Artist - Album (2020) [MP3]").
- Inside the album folder, sub-folders are kept (CD1, CD2...). Only music files and cover images are copied.
  A FLAC track that cannot be converted is skipped (logged); the rest of the album is imported.
- An album folder that already exists is never overwritten nor merged: the album is skipped (logged).
- The copy is made in STAGING (same disk as the library, outside it) then moved in one rename, so the music
  server never indexes a half-copied album. Free space is checked first.
- Each torrent is handled once (DONE_LIST): deleting the torrent keeps the music, deleting the album from
  the library does not bring it back. To import a torrent again, remove its line from DONE_LIST.
- Nothing happens while one of the disks is not mounted; the entries wait for the next run.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
import contextlib
from collections import Counter
from concurrent.futures import ThreadPoolExecutor

ENV = os.environ.get
DOWNLOADS = ENV("DOWNLOADS", "/mnt/usb/WD2To/downloads")
CONTAINER_DOWNLOADS = ENV("CONTAINER_DOWNLOADS", "/downloads")
FINISHED_LIST = ENV("FINISHED_LIST", "/home/pi/.config/qbittorrent/finished-torrents.txt")
DONE_LIST = ENV("DONE_LIST", "/home/pi/.local/state/jukepi/imported-torrents.txt")
MUSIC_ROOT = ENV("MUSIC_ROOT", "/mnt/music/music")
STAGING = ENV("STAGING", "/mnt/music/.jukepi-incoming")
SCAN_URL = ENV("SCAN_URL", "http://localhost:8765/scan")
REQUIRE_MOUNTED = ENV("REQUIRE_MOUNTED", "1") != "0"  # 0: tests in a temporary folder
VARIOUS = ENV("VARIOUS_ARTISTS", "Various Artists")
IMAGES = (".jpg", ".jpeg", ".png", ".webp")
AUDIO = (".mp3", ".flac")
MP3_BITRATE = ENV("MP3_BITRATE", "320k")
CONVERT_JOBS = int(ENV("CONVERT_JOBS", "2"))  # FLAC conversions at once (the Pi has other work)
MARGIN = 1024**3  # keep 1 GiB free


def log(msg):
    print(msg, flush=True)


def warn(msg):
    print(f"WARNING: {msg}", file=sys.stderr, flush=True)


# ---------- names ----------

def clean_name(name, fallback):
    """A folder name safe on NTFS/Windows: no reserved characters, no trailing dots or spaces."""
    name = re.sub(r'[\\/:*?"<>|\x00-\x1f]', "_", name or "")
    name = re.sub(r"\s+", " ", name).strip().rstrip(". ").strip()
    if len(name) > 120:
        name = name[:120].rstrip(". ").strip()
    return name or fallback


RELEASE_NOISE = re.compile(
    r"\s*[\[(](?:\d{4}|mp3|flac|web|cd|vinyl|\d{3}\s*k(?:bps)?|v0|v2|320|256|192|\d{4}\s*remaster(?:ed)?|"
    r"(?:deluxe|expanded|remastered)(?: edition)?)[^\])]*[\])]",
    re.IGNORECASE,
)


def split_folder_name(folder):
    """'Artist - Album (2020) [MP3 320]' -> ('Artist', 'Album'). Without ' - ': (None, folder)."""
    name = RELEASE_NOISE.sub("", folder).strip()
    name = re.sub(r"\s*-\s*(?:\d{4}|mp3|flac|web)\s*$", "", name, flags=re.IGNORECASE).strip()
    if " - " in name:
        artist, album = name.split(" - ", 1)
        album = re.sub(r"^(?:\d{4})\s*-\s*", "", album).strip()  # 'Artist - 2020 - Album'
        return artist.strip() or None, album.strip() or None
    return None, name or folder


# ---------- tags ----------

def read_tags(path):
    """{'album', 'artist', 'album_artist'} (lowercase keys, empty when missing) via ffprobe."""
    try:
        out = subprocess.run(
            ["ffprobe", "-v", "error", "-print_format", "json", "-show_entries", "format_tags", path],
            capture_output=True, text=True, timeout=30, check=False,
        ).stdout
        tags = {k.lower(): v for k, v in (json.loads(out or "{}").get("format", {}).get("tags", {}) or {}).items()}
    except (subprocess.SubprocessError, ValueError, OSError):
        tags = {}
    pick = lambda *keys: next((tags[k].strip() for k in keys if tags.get(k, "").strip()), "")
    return {
        "album": pick("album"),
        "artist": pick("artist"),
        "album_artist": pick("album_artist", "albumartist", "album artist"),
    }


DISC_DIR = re.compile(r"^(?:cd|disc|disk|disque)\s*\d+$", re.IGNORECASE)
FEAT = re.compile(r"\s+(?:feat\.?|ft\.?|featuring|with)\s+.*$", re.IGNORECASE)


def main_artist(items):
    """Album artist tag if any; else the track artist of more than half of the tracks ('feat.' ignored);
    else Various Artists. None when no track has an artist tag."""
    album_artists = Counter(t["album_artist"] for _, t in items if t["album_artist"])
    if album_artists:
        return album_artists.most_common(1)[0][0]
    artists = Counter(FEAT.sub("", t["artist"]).strip() for _, t in items if t["artist"])
    if not artists:
        return None
    top, n = artists.most_common(1)[0]
    return top if n * 2 > len(items) else VARIOUS


def plan_albums(root, tags_of=read_tags):
    """Groups the MP3/FLAC files under `root` (a folder, or a single file) into albums.

    Returns [(artist, album, base_dir, [track paths], copy_images)]: files are copied relative to base_dir
    (their common folder, so CD1/CD2 sub-folders are kept); copy_images is False for a single file (its
    folder is the downloads folder, its images are not its cover).
    """
    single = os.path.isfile(root)
    if single:
        files = [root]
        t_artist, t_album = split_folder_name(os.path.splitext(os.path.basename(root))[0])
    else:
        files = sorted(os.path.join(d, f) for d, _, fs in os.walk(root) for f in fs if f.lower().endswith(AUDIO))
        t_artist, t_album = split_folder_name(os.path.basename(root.rstrip("/")))
    groups = {}
    for f in files:
        t = tags_of(f)
        if t["album"]:
            key = "tag:" + t["album"].casefold()
        else:  # no album tag: the folder holding the file is the album (a CD1/CD2 folder: its parent)
            d = os.path.dirname(f)
            if DISC_DIR.match(os.path.basename(d)) and d.rstrip("/") != root.rstrip("/"):
                d = os.path.dirname(d)
            key = "dir:" + d
        groups.setdefault(key, []).append((f, t))

    plans = []
    for items in groups.values():
        if any(f.lower().endswith(".mp3") for f, _ in items):  # same album as MP3 and FLAC: the MP3 is enough
            items = [(f, t) for f, t in items if f.lower().endswith(".mp3")]
        paths = [f for f, _ in items]
        base = os.path.commonpath([os.path.dirname(p) for p in paths])
        if single or base.rstrip("/") == root.rstrip("/"):
            g_artist, g_album = t_artist, t_album
        else:
            a, g_album = split_folder_name(os.path.basename(base))
            if a and re.fullmatch(r"\d{4}", a):  # '2001 - Album' inside an artist's discography
                a = None
            g_artist = a or t_artist
        artist = main_artist(items) or g_artist or "Unknown Artist"
        tagged = Counter(t["album"] for _, t in items if t["album"]).most_common(1)
        if tagged:
            album = tagged[0][0]
        elif single:
            album = "Singles"
        else:
            album = g_album or "Unknown Album"
        plans.append((clean_name(artist, "Unknown Artist"), clean_name(album, "Unknown Album"), base, paths, not single))
    return plans


# ---------- copy ----------

def mount_point(path):
    path = os.path.realpath(path)
    while not os.path.ismount(path):
        path = os.path.dirname(path)
    return path


def on_mounted_disk(path):
    """True when `path` is on a mounted disk, not on the root filesystem under an empty mount point.
    (REQUIRE_MOUNTED=0 skips the mount part: tests in a temporary folder.)"""
    if not os.path.exists(path):
        return False
    return not REQUIRE_MOUNTED or mount_point(path) != "/"


def free_bytes(path):
    st = os.statvfs(path)
    return st.f_bavail * st.f_frsize


def sample_rate(path):
    try:
        out = subprocess.run(
            ["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries", "stream=sample_rate",
             "-of", "default=nw=1:nk=1", path],
            capture_output=True, text=True, timeout=30, check=False,
        ).stdout.strip()
        return int(out) if out.isdigit() else 0
    except (subprocess.SubprocessError, OSError):
        return 0


def to_mp3(src, target):
    """FLAC -> MP3 (MP3_BITRATE), tags and embedded cover kept. Returns None or an error message."""
    base_cmd = ["ffmpeg", "-v", "error", "-nostdin", "-y", "-i", src, "-map", "0:a:0"]
    audio = ["-c:a", "libmp3lame", "-b:a", MP3_BITRATE, "-map_metadata", "0", "-id3v2_version", "3"]
    if sample_rate(src) > 48000:  # MP3 stops at 48 kHz (hi-res FLAC)
        audio += ["-ar", "48000"]
    with_cover = ["-map", "0:v?", "-c:v", "copy", "-disposition:v", "attached_pic"]
    err = ""
    for extra in (with_cover, []):  # a cover MP3 cannot hold: retry without it
        try:
            r = subprocess.run(base_cmd + extra + audio + ["-f", "mp3", target],
                               capture_output=True, text=True, timeout=1800, check=False)
        except (subprocess.SubprocessError, OSError) as e:
            return str(e)
        if r.returncode == 0:
            return None
        err = (r.stderr or "").strip().splitlines()[-1:] or ["ffmpeg failed"]
        err = err[0]
    return err


def copy_album(artist, album, base, tracks, copy_images=True):
    """Copies one album into MUSIC_ROOT/artist/album, FLAC tracks converted to MP3.
    Returns 'imported', 'exists', 'nospace' / 'retry' (try again at the next run) or 'failed' (no track usable:
    not retried)."""
    dest = os.path.join(MUSIC_ROOT, artist, album)
    if os.path.exists(dest):
        log(f"already in the library, skipped: {artist}/{album}")
        return "exists"
    images = []
    if copy_images:  # covers: images in the album's folders (base folder + every folder holding its files)
        dirs = {base} | {os.path.dirname(p) for p in tracks}
        images = sorted(os.path.join(d, f) for d in dirs for f in os.listdir(d) if f.lower().endswith(IMAGES))
    flacs = [f for f in tracks if f.lower().endswith(".flac")]
    # MP3 320k is about a third of a FLAC: count half to be safe
    size = sum(os.path.getsize(f) for f in tracks + images if f not in flacs) + sum(os.path.getsize(f) for f in flacs) // 2
    if free_bytes(MUSIC_ROOT) < size + MARGIN:
        warn(f"not enough space on the music disk for {artist}/{album} ({size // 2**20} MB), will retry")
        return "nospace"
    stage = os.path.join(STAGING, uuid.uuid4().hex)

    def target_of(f):
        rel = os.path.relpath(f, base)
        if f in flacs:
            rel = os.path.splitext(rel)[0] + ".mp3"
        t = os.path.join(stage, *[clean_name(p, "_") for p in rel.split(os.sep)])
        os.makedirs(os.path.dirname(t), exist_ok=True)
        return t

    failed = []
    try:
        for f in tracks + images:
            if f not in flacs:
                shutil.copy2(f, target_of(f))
        if flacs:
            with ThreadPoolExecutor(max_workers=max(1, CONVERT_JOBS)) as pool:
                jobs = {f: pool.submit(to_mp3, f, target_of(f)) for f in flacs}
            # A source file that vanished (or a downloads disk that dropped) is not a broken FLAC: give the
            # whole album up and retry later, instead of importing half of it.
            if any(job.result() for job in jobs.values()) and (
                not on_mounted_disk(DOWNLOADS) or any(job.result() and not os.path.exists(f) for f, job in jobs.items())
            ):
                shutil.rmtree(stage, ignore_errors=True)
                warn(f"source files of {artist}/{album} are gone (downloads disk unplugged?), will retry")
                return "retry"
            for f, job in jobs.items():
                if job.result():
                    failed.append(f)
                    warn(f"FLAC not converted, skipped: {os.path.relpath(f, base)} ({job.result()})")
                    with contextlib.suppress(OSError):
                        os.remove(target_of(f))
        if len(failed) == len(tracks):
            shutil.rmtree(stage, ignore_errors=True)
            warn(f"no track could be imported for {artist}/{album}")
            return "failed"
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        os.rename(stage, dest)
    except OSError:
        shutil.rmtree(stage, ignore_errors=True)
        raise
    n = len(tracks) - len(failed)
    converted = len(flacs) - len(failed)
    note = f", {converted} converted from FLAC" if converted else ""
    log(f"imported: {artist}/{album} ({n} tracks{note})")
    return "imported"


def request_scan():
    """202 = scan started; 409 = one is running and may have started before this change: wait, retry."""
    for _ in range(24):
        try:
            with urllib.request.urlopen(urllib.request.Request(SCAN_URL, method="POST"), timeout=10) as r:
                if r.status == 202:
                    log("music server rescan started")
                    return
        except urllib.error.HTTPError as e:
            if e.code != 409:
                warn(f"music server at {SCAN_URL} answered {e.code}")
                return
        except (urllib.error.URLError, OSError) as e:
            warn(f"music server unreachable at {SCAN_URL}: {e}")
            return
        time.sleep(5)
    warn("music server still busy after 2 minutes: the new albums show up at its next scan")


def host_path(path):
    if path == CONTAINER_DOWNLOADS or path.startswith(CONTAINER_DOWNLOADS.rstrip("/") + "/"):
        path = DOWNLOADS.rstrip("/") + path[len(CONTAINER_DOWNLOADS.rstrip("/")):]
    return path.rstrip("/")


def main():
    if not os.path.isdir(DOWNLOADS) or not on_mounted_disk(DOWNLOADS):
        log(f"downloads disk not mounted ({DOWNLOADS}): nothing to do")
        return 0
    if not on_mounted_disk(MUSIC_ROOT):
        log(f"music disk not mounted ({MUSIC_ROOT}): nothing to do")
        return 0
    if not os.path.exists(FINISHED_LIST):
        return 0
    os.makedirs(os.path.dirname(DONE_LIST), exist_ok=True)
    shutil.rmtree(STAGING, ignore_errors=True)  # copies cut by a crash or a reboot
    os.makedirs(STAGING, exist_ok=True)
    done = set()
    if os.path.exists(DONE_LIST):
        with open(DONE_LIST, encoding="utf-8") as f:
            done = {l.rstrip("\n") for l in f}

    imported = False
    with open(FINISHED_LIST, encoding="utf-8", errors="surrogateescape") as f:
        entries = [l.rstrip("\n") for l in f if l.strip()]
    for entry in dict.fromkeys(entries):  # in order, without repeats
        if entry in done:
            continue
        src = host_path(entry)
        finished = True
        if os.path.isdir(src) or (os.path.isfile(src) and src.lower().endswith(AUDIO)):
            for artist, album, base, tracks, copy_images in plan_albums(src):
                try:
                    result = copy_album(artist, album, base, tracks, copy_images)
                except OSError as e:
                    warn(f"copy of {artist}/{album} failed ({e}), will retry")
                    result = "nospace"
                imported |= result == "imported"
                finished &= result not in ("nospace", "retry")
        if finished:  # also: no MP3 inside, or deleted before we got to it
            with open(DONE_LIST, "a", encoding="utf-8", errors="surrogateescape") as f:
                f.write(entry + "\n")
            done.add(entry)
    shutil.rmtree(STAGING, ignore_errors=True)
    if imported:
        request_scan()
    return 0


if __name__ == "__main__":
    sys.exit(main())
