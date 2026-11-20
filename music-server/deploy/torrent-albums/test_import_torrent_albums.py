"""Tests for import-torrent-albums.py:  python3 -m unittest discover -s music-server/deploy/torrent-albums
The end-to-end test needs ffmpeg/ffprobe (skipped without)."""

import importlib.util
import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))


def load(env):
    """Loads the script as a module with these environment variables (it reads them at import)."""
    old = {k: os.environ.get(k) for k in env}
    os.environ.update(env)
    try:
        spec = importlib.util.spec_from_file_location("import_torrent_albums", os.path.join(HERE, "import-torrent-albums.py"))
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        return mod
    finally:
        for k, v in old.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


M = load({})


def touch(path, data=b"x"):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


class Names(unittest.TestCase):
    def test_split_folder_name(self):
        s = M.split_folder_name
        self.assertEqual(s("Oasis - Definitely Maybe (1994) [MP3 320]"), ("Oasis", "Definitely Maybe"))
        self.assertEqual(s("Daft Punk - 2001 - Discovery"), ("Daft Punk", "Discovery"))
        self.assertEqual(s("Björk - Homogenic [WEB] (Remastered)"), ("Björk", "Homogenic"))
        self.assertEqual(s("Some Album"), (None, "Some Album"))
        self.assertEqual(s("Air - Moon Safari - 1998"), ("Air", "Moon Safari"))

    def test_clean_name(self):
        c = M.clean_name
        self.assertEqual(c('AC/DC: "Live"?', "x"), "AC_DC_ _Live__")
        self.assertEqual(c("  Album...  ", "x"), "Album")
        self.assertEqual(c("", "Unknown"), "Unknown")
        self.assertEqual(len(c("a" * 300, "x")), 120)


class Planning(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.tags = {}

    def tearDown(self):
        shutil.rmtree(self.dir)

    def mp3(self, rel, album="", artist="", album_artist=""):
        p = os.path.join(self.dir, rel)
        touch(p)
        self.tags[p] = {"album": album, "artist": artist, "album_artist": album_artist}
        return p

    def plan(self, rel):
        return sorted(
            (a, b, os.path.relpath(base, self.dir), len(files), img)
            for a, b, base, files, img in M.plan_albums(os.path.join(self.dir, rel), self.tags.get)
        )

    def test_tagged_album(self):
        self.mp3("T/01.mp3", "Definitely Maybe", "Oasis")
        self.mp3("T/02.mp3", "Definitely Maybe", "Oasis")
        self.assertEqual(self.plan("T"), [("Oasis", "Definitely Maybe", "T", 2, True)])

    def test_multi_disc_keeps_cd_folders(self):
        self.mp3("T/CD1/01.mp3", "Double", "Artist")
        self.mp3("T/CD2/01.mp3", "Double", "Artist")
        self.assertEqual(self.plan("T"), [("Artist", "Double", "T", 2, True)])

    def test_compilation_and_feat(self):
        self.mp3("C/01.mp3", "Hits", "A")
        self.mp3("C/02.mp3", "Hits", "B")
        self.mp3("C/03.mp3", "Hits", "C")
        self.assertEqual(self.plan("C")[0][:2], ("Various Artists", "Hits"))
        self.mp3("D/01.mp3", "Duo", "A")
        self.mp3("D/02.mp3", "Duo", "B")
        self.assertEqual(self.plan("D")[0][:2], ("Various Artists", "Duo"))  # 1 of 2 is not a majority
        self.mp3("F/01.mp3", "Album", "Drake")
        self.mp3("F/02.mp3", "Album", "Drake feat. Rihanna")
        self.mp3("F/03.mp3", "Album", "Other")
        self.assertEqual(self.plan("F")[0][:2], ("Drake", "Album"))
        self.mp3("G/01.mp3", "Album", "A", album_artist="The Band")
        self.mp3("G/02.mp3", "Album", "B", album_artist="The Band")
        self.assertEqual(self.plan("G")[0][:2], ("The Band", "Album"))

    def test_untagged_discography(self):
        self.mp3("Radiohead - Discography/1997 - OK Computer/01.mp3")
        self.mp3("Radiohead - Discography/2000 - Kid A/01.mp3")
        self.mp3("Radiohead - Discography/2000 - Kid A/02.mp3")
        self.assertEqual(
            self.plan("Radiohead - Discography"),
            [
                ("Radiohead", "Kid A", "Radiohead - Discography/2000 - Kid A", 2, True),
                ("Radiohead", "OK Computer", "Radiohead - Discography/1997 - OK Computer", 1, True),
            ],
        )

    def test_untagged_cd_folders_are_one_album(self):
        self.mp3("Air - Moon Safari (1998)/CD1/01.mp3")
        self.mp3("Air - Moon Safari (1998)/CD2/01.mp3")
        self.assertEqual(self.plan("Air - Moon Safari (1998)"), [("Air", "Moon Safari", "Air - Moon Safari (1998)", 2, True)])

    def test_single_file(self):
        self.mp3("Artist - Song.mp3")
        self.assertEqual(self.plan("Artist - Song.mp3"), [("Artist", "Singles", ".", 1, False)])
        self.mp3("x.mp3", "Real Album", "Someone")
        self.assertEqual(self.plan("x.mp3"), [("Someone", "Real Album", ".", 1, False)])


@unittest.skipUnless(shutil.which("ffmpeg") and shutil.which("ffprobe"), "ffmpeg not installed")
class EndToEnd(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp()
        j = lambda *p: os.path.join(self.root, *p)
        self.downloads, self.music = j("downloads"), j("music")
        os.makedirs(self.downloads)
        os.makedirs(self.music)
        self.finished, self.done = j("finished.txt"), j("state", "done.txt")
        self.m = load({
            "DOWNLOADS": self.downloads, "CONTAINER_DOWNLOADS": "/downloads", "FINISHED_LIST": self.finished,
            "DONE_LIST": self.done, "MUSIC_ROOT": self.music, "STAGING": j("incoming"),
            "SCAN_URL": "http://127.0.0.1:9/scan", "REQUIRE_MOUNTED": "0",
        })

    def tearDown(self):
        shutil.rmtree(self.root)

    def make_mp3(self, rel, **tags):
        p = os.path.join(self.downloads, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        meta = [a for k, v in tags.items() for a in ("-metadata", f"{k}={v}")]
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=0.2", *meta, p], check=True)

    def finish(self, name):
        with open(self.finished, "a") as f:
            f.write(f"/downloads/{name}\n")

    def files(self):
        return sorted(os.path.relpath(os.path.join(d, f), self.music) for d, _, fs in os.walk(self.music) for f in fs)

    def test_import(self):
        t = "Oasis - Definitely Maybe (1994) [MP3]"
        self.make_mp3(f"{t}/CD1/01 - Rock.mp3", artist="Oasis", album="Definitely Maybe")
        self.make_mp3(f"{t}/CD2/01 - Live.mp3", artist="Oasis", album="Definitely Maybe")
        touch(os.path.join(self.downloads, t, "cover.jpg"))
        touch(os.path.join(self.downloads, t, "release.nfo"))
        self.finish(t)
        self.finish("Not Music")  # no MP3: just marked as done
        touch(os.path.join(self.downloads, "Not Music", "movie.mkv"))
        self.m.main()
        self.assertEqual(self.files(), [
            "Oasis/Definitely Maybe/CD1/01 - Rock.mp3",
            "Oasis/Definitely Maybe/CD2/01 - Live.mp3",
            "Oasis/Definitely Maybe/cover.jpg",
        ])
        with open(self.done) as f:
            self.assertEqual(f.read().splitlines(), [f"/downloads/{t}", "/downloads/Not Music"])
        self.assertFalse(os.path.exists(os.path.join(self.root, "incoming")))

        # handled once: removing the album from the library does not bring it back
        shutil.rmtree(os.path.join(self.music, "Oasis"))
        self.m.main()
        self.assertEqual(self.files(), [])

        # an album already in the library is never overwritten
        os.makedirs(os.path.join(self.music, "Air", "Moon Safari"))
        touch(os.path.join(self.music, "Air", "Moon Safari", "mine.mp3"), b"keep")
        self.make_mp3("Air - Moon Safari/01.mp3", artist="Air", album="Moon Safari")
        self.finish("Air - Moon Safari")
        self.m.main()
        self.assertEqual(self.files(), ["Air/Moon Safari/mine.mp3"])


if __name__ == "__main__":
    unittest.main()
