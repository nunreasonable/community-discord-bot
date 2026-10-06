#!/usr/bin/env python3
"""Testes do daeese_presence (so stdlib).

    python3 -m unittest -v test_presence      (dentro de presence/)
"""

import copy
import io
import json
import os
import shutil
import socket
import struct
import subprocess
import tempfile
import time
import unittest
import urllib.error

import daeese_presence as dp


BASE_CONFIG = {
    "applicationId": "1403153848507301939",
    "largeImage": "https://daeese.me/filearchive/onese.jpeg",
    "largeText": "daeese.me",
    "smallImage": "https://daeese.me/filearchive/widget/rp-small.png",
    "smallText": "C# / .NET",
    "details": "daeese.me",
    "state": "Jr. Software Engineer",
    "activityType": 3,
    "showElapsed": False,
    "buttons": [
        {"label": "daeese.me", "url": "https://daeese.me"},
        {"label": "Sollarety", "url": "https://apps.daeese.me/sollarety/"},
    ],
}


def make_config(**states):
    config = copy.deepcopy(BASE_CONFIG)
    if states:
        config["states"] = states
    return dp.validate_config(config)


def coding_info(repo_key="/home/u/proj", public=None, language="Python", badge="python"):
    return {
        "repo_key": repo_key,
        "workdir": repo_key,
        "language": language,
        "badge": badge,
        "public": public,
        "last_change": 0,
        "candidates": 1,
        "detector": "process-scan+git",
    }


def vm_info(key="libvirt:qemu:///system:win11", name="Windows 11", badge="windows11", started=None):
    return {"key": key, "os": {"name": name, "badge": badge}, "started": started, "detector": "libvirt"}


class LibosinfoMappingTest(unittest.TestCase):
    def no_query(self, _os_id):
        return None

    def test_known_ids(self):
        cases = {
            "http://microsoft.com/win/11": ("Windows 11", "windows11"),
            "http://microsoft.com/win/10": ("Windows 10", "windows10"),
            "http://gentoo.org/gentoo/rolling": ("Gentoo", "gentoo"),
            "http://fedoraproject.org/fedora/40": ("Fedora 40", "fedora"),
            "http://fedoraproject.org/fedora/rawhide": ("Fedora Rawhide", "fedora"),
            "http://ubuntu.com/ubuntu/24.04": ("Ubuntu 24.04", "ubuntu"),
            "http://debian.org/debian/12": ("Debian 12", "debian"),
            "http://archlinux.org/archlinux/rolling": ("Arch Linux", "arch"),
            "http://apple.com/macos/14": ("macOS 14", "macos"),
            "http://freebsd.org/freebsd/14.0": ("FreeBSD 14.0", "freebsd"),
            "http://libosinfo.org/linux/2022": ("Linux", "linux"),
        }
        for os_id, (name, badge) in cases.items():
            with self.subTest(os_id=os_id):
                self.assertEqual(dp.os_from_libosinfo(os_id, query=self.no_query), {"name": name, "badge": badge})

    def test_other_windows_uses_osinfo_name(self):
        info = dp.os_from_libosinfo("http://microsoft.com/win/8.1", query=lambda _: "Microsoft Windows 8.1")
        self.assertEqual(info, {"name": "Windows 8.1", "badge": "windows"})
        info = dp.os_from_libosinfo("http://microsoft.com/win/7", query=self.no_query)
        self.assertEqual(info, {"name": "Windows 7", "badge": "windows"})

    def test_unknown_ids(self):
        info = dp.os_from_libosinfo("http://nixos.org/nixos/24.05", query=lambda _: "NixOS 24.05")
        self.assertEqual(info, {"name": "NixOS 24.05", "badge": "linux"})
        info = dp.os_from_libosinfo("http://opensuse.org/opensuse/15.6", query=self.no_query)
        self.assertEqual(info, {"name": "Opensuse 15.6", "badge": "linux"})
        info = dp.os_from_libosinfo("http://openbsd.org/openbsd/7.5", query=self.no_query)
        self.assertEqual(info["badge"], "generic")
        self.assertEqual(dp.os_from_libosinfo("garbage", query=self.no_query), dp.UNKNOWN_OS)
        self.assertEqual(dp.os_from_libosinfo("http://libosinfo.org/unknown", query=self.no_query), dp.UNKNOWN_OS)

    def test_badges_are_in_contract(self):
        for os_id in ("http://microsoft.com/win/2k22", "http://example.org/thing/1", "http://apple.com/macosx/10.15"):
            self.assertIn(dp.os_from_libosinfo(os_id, query=self.no_query)["badge"], dp.OS_BADGES)

    def test_xml_extraction(self):
        xml = """<domain type='kvm'><name>win11</name><metadata>
          <libosinfo:libosinfo xmlns:libosinfo="http://libosinfo.org/xmlns/libvirt/domain/1.0">
            <libosinfo:os id="http://microsoft.com/win/11"/>
          </libosinfo:libosinfo></metadata></domain>"""
        self.assertEqual(dp.libosinfo_id_from_xml(xml), "http://microsoft.com/win/11")
        self.assertIsNone(dp.libosinfo_id_from_xml("<domain><name>x</name></domain>"))
        self.assertIsNone(dp.libosinfo_id_from_xml("not xml <"))

    def test_hints(self):
        self.assertEqual(dp.os_from_hint("win11-gpu")["badge"], "windows11")
        self.assertEqual(dp.os_from_hint("Windows 10")["badge"], "windows10")
        self.assertEqual(dp.os_from_hint("gentoo")["name"], "Gentoo")
        self.assertIsNone(dp.os_from_hint("my-box")["name"])
        self.assertEqual(dp.os_from_vmware_guest("windows9-64")["name"], "Windows 10")
        self.assertEqual(dp.os_from_vmware_guest("other6xlinux-64")["badge"], "linux")
        self.assertEqual(dp.os_from_vbox_type("ArchLinux_64")["badge"], "arch")

    def test_qemu_cmdline(self):
        argv = ["/usr/bin/qemu-system-x86_64", "-name", "guest=win11,debug-threads=on",
                "-cpu", "host,hv-relaxed=on,hv-vapic=on"]
        self.assertEqual(dp.qemu_guest_name(argv), "win11")
        self.assertTrue(dp.qemu_hint_windows(argv))
        self.assertEqual(dp.qemu_guest_name(["qemu-system-x86_64", "-name", "plain"]), "plain")
        self.assertFalse(dp.qemu_hint_windows(["qemu-system-x86_64", "-cpu", "host"]))


class RemoteParsingTest(unittest.TestCase):
    def test_github_forms(self):
        cases = {
            "git@github.com:nunreasonable/community-discord-bot.git": ("nunreasonable", "community-discord-bot"),
            "git@github.com:nunreasonable/nunreasonable.github.io.git": ("nunreasonable", "nunreasonable.github.io"),
            "ssh://git@github.com/owner/repo.git": ("owner", "repo"),
            "ssh://git@github.com:22/owner/repo": ("owner", "repo"),
            "https://github.com/owner/repo": ("owner", "repo"),
            "https://github.com/owner/repo.git/": ("owner", "repo"),
            "https://user@github.com/owner/repo.git": ("owner", "repo"),
            "https://www.github.com/owner/repo": ("owner", "repo"),
            "git://github.com/owner/repo.git": ("owner", "repo"),
            "git@github.com:1234/repo": ("1234", "repo"),
            "  git@github.com:o/r.git\n": ("o", "r"),
        }
        for url, expected in cases.items():
            with self.subTest(url=url):
                self.assertEqual(dp.parse_github_remote(url), expected)

    def test_not_github(self):
        for url in ("git@gitlab.com:o/r.git", "https://github.com.evil.com/o/r", "https://github.com/o",
                    "/srv/git/repo.git", "", None, "https://github.com/o/r/tree/main"):
            with self.subTest(url=url):
                self.assertIsNone(dp.parse_github_remote(url))


class LanguageTest(unittest.TestCase):
    def test_extension_table(self):
        cases = {
            "Program.cs": "csharp", "a.cpp": "cpp", "a.cc": "cpp", "a.hpp": "cpp", "a.hxx": "cpp",
            "a.c": "c", "a.h": "c", "lib.rs": "rust", "x.py": "python", "x.js": "javascript",
            "x.mjs": "javascript", "x.cjs": "javascript", "x.ts": "typescript", "x.tsx": "typescript",
            "x.luau": "luau", "x.lua": "lua", "index.html": "html", "s.css": "css", "r.sh": "shell",
            "r.bash": "shell", "c.fish": "shell", "Main.qml": "qml", "m.go": "go", "M.java": "java",
            "m.kt": "kotlin", "README.md": "markdown", "dir/UPPER.CS": "csharp",
        }
        for path, lang in cases.items():
            with self.subTest(path=path):
                self.assertEqual(dp.language_for_path(path), lang)
        self.assertIsNone(dp.language_for_path("config.json"))
        self.assertIsNone(dp.language_for_path("Makefile"))
        for lang in set(dp.EXTENSION_LANGUAGES.values()):
            self.assertIn(lang, dp.LANG_BADGES)
            self.assertIn(lang, dp.LANGUAGE_NAMES)

    def test_majority(self):
        files = [("a.py", 10), ("b.py", 11), ("c.cs", 50), ("x.json", 99)]
        self.assertEqual(dp.majority_language(files), "python")
        # Empate: ganha a linguagem do arquivo mais recente.
        self.assertEqual(dp.majority_language([("a.py", 10), ("c.cs", 50)]), "csharp")
        self.assertIsNone(dp.majority_language([("x.json", 1), ("", 2)]))

    def test_dominant_prefers_code_over_markdown(self):
        paths = ["README.md", "docs/a.md", "docs/b.md", "src/main.rs"]
        self.assertEqual(dp.dominant_language(paths), "rust")
        self.assertEqual(dp.dominant_language(["README.md"]), "markdown")
        self.assertIsNone(dp.dominant_language(["LICENSE"]))

    def test_status_parsing(self):
        out = " M src/a.py\0?? new file.cs\0R  dst.rs\0src.rs\0A  b.ts\0"
        self.assertEqual(dp.parse_status_z(out), ["src/a.py", "new file.cs", "dst.rs", "b.ts"])
        self.assertEqual(dp.parse_status_z(""), [])


class StateTest(unittest.TestCase):
    def test_priority(self):
        timeline = dp.Timeline()
        self.assertEqual(dp.select_state([], None, timeline, 100)["kind"], "idle")
        self.assertEqual(dp.select_state([], coding_info(), timeline, 100)["kind"], "coding")
        snap = dp.select_state([vm_info()], coding_info(), timeline, 100)
        self.assertEqual(snap["kind"], "vm")
        self.assertIsNotNone(snap["coding"])
        later = vm_info(key="k2", name="Gentoo", badge="gentoo", started=90)
        snap = dp.select_state([vm_info(started=50), later], None, dp.Timeline(), 100)
        self.assertEqual(snap["vm_count"], 2)
        # A primaria e a que esta rodando ha mais tempo.
        self.assertEqual(snap["vm"]["os"]["name"], "Windows 11")

    def test_coding_timestamp_stable_and_resets(self):
        timeline = dp.Timeline()
        first = dp.select_state([], coding_info("/r/a"), timeline, 1000)
        later = dp.select_state([], coding_info("/r/a", language="Rust", badge="rust"), timeline, 1060)
        self.assertEqual(first["since"], 1000)
        self.assertEqual(later["since"], 1000)  # trocar de linguagem nao zera
        other = dp.select_state([], coding_info("/r/b"), timeline, 1120)
        self.assertEqual(other["since"], 1120)  # trocar de repositorio zera
        dp.select_state([], None, timeline, 1180)
        back = dp.select_state([], coding_info("/r/b"), timeline, 1240)
        self.assertEqual(back["since"], 1240)

    def test_vm_timestamp(self):
        timeline = dp.Timeline()
        snap = dp.select_state([vm_info(started=500)], None, timeline, 1000)
        self.assertEqual(snap["since"], 500)  # inicio do processo da VM
        snap = dp.select_state([vm_info(started=500)], None, timeline, 2000)
        self.assertEqual(snap["since"], 500)
        snap = dp.select_state([vm_info(key="new")], None, timeline, 3000)
        self.assertEqual(snap["since"], 3000)  # sem processo: primeira vez vista


class ActivityTest(unittest.TestCase):
    def setUp(self):
        self.config = make_config()

    def build(self, vms, coding, now=1000.0, timeline=None):
        timeline = timeline or dp.Timeline()
        return dp.build_activity(self.config, dp.select_state(vms, coding, timeline, now), started_at=1.0)

    def assert_discord_limits(self, activity):
        buttons = activity.get("buttons", [])
        self.assertLessEqual(len(buttons), 2)
        for button in buttons:
            self.assertLessEqual(len(button["label"]), 32)
            self.assertTrue(button["url"].startswith("https://"))
        for key in ("details", "state"):
            if key in activity:
                self.assertTrue(2 <= len(activity[key]) <= 128)

    def test_idle_is_the_configured_card(self):
        activity = self.build([], None)
        self.assertEqual(activity, {
            "type": 3,
            "details": "daeese.me",
            "state": "Jr. Software Engineer",
            "assets": {
                "large_image": "https://daeese.me/filearchive/onese.jpeg",
                "large_text": "daeese.me",
                "small_image": "https://daeese.me/filearchive/widget/rp-small.png",
                "small_text": "C# / .NET",
            },
            "buttons": BASE_CONFIG["buttons"],
        })
        self.assertNotIn("status_display_type", activity)

    def test_coding_public(self):
        public = {"name": "community-discord-bot", "url": "https://github.com/nunreasonable/community-discord-bot"}
        activity = self.build([], coding_info(public=public, language="C#", badge="csharp"))
        self.assertEqual(activity["type"], 3)
        self.assertEqual(activity["status_display_type"], 2)
        self.assertEqual(activity["details"], "Charting new stars")
        self.assertEqual(activity["state"], "Writing C# in community-discord-bot")
        self.assertEqual(activity["assets"]["large_image"], "https://daeese.me/filearchive/presence/large/code.png")
        self.assertEqual(activity["assets"]["small_image"], "https://daeese.me/filearchive/presence/lang/csharp.png")
        self.assertEqual(activity["assets"]["small_text"], "C#")
        self.assertEqual(activity["timestamps"], {"start": 1000000})
        self.assertEqual(activity["buttons"], [
            {"label": "daeese.me", "url": "https://daeese.me"},
            {"label": "community-discord-bot on GitHub", "url": public["url"]},
        ])
        self.assert_discord_limits(activity)

    def test_coding_private_never_leaks(self):
        coding = coding_info(repo_key="/home/u/secret-thing")
        activity = self.build([], coding)
        self.assertEqual(activity["state"], "Writing Python in a private project")
        self.assertEqual(activity["buttons"], BASE_CONFIG["buttons"])
        self.assertNotIn("secret-thing", json.dumps(activity))
        self.assert_discord_limits(activity)

    def test_long_repo_name_button(self):
        public = {"name": "nunreasonable.github.io", "url": "https://github.com/nunreasonable/nunreasonable.github.io"}
        activity = self.build([], coding_info(public=public, language="HTML", badge="html"))
        self.assertEqual(activity["buttons"][1]["label"], "GitHub: nunreasonable.github.io")
        public = {"name": "x" * 60, "url": "https://github.com/o/" + "x" * 60}
        activity = self.build([], coding_info(public=public))
        self.assertEqual(activity["buttons"][1]["label"], "View on GitHub")
        self.assert_discord_limits(activity)

    def test_vm_only(self):
        activity = self.build([vm_info(started=400)], None)
        self.assertEqual(activity["status_display_type"], 2)
        self.assertEqual(activity["details"], "Orbiting Windows 11")
        self.assertEqual(activity["state"], "in a virtual machine")
        self.assertEqual(activity["assets"]["large_image"], "https://daeese.me/filearchive/presence/large/orbit.png")
        self.assertEqual(activity["assets"]["small_image"], "https://daeese.me/filearchive/presence/os/windows11.png")
        self.assertEqual(activity["assets"]["small_text"], "Windows 11")
        self.assertEqual(activity["timestamps"], {"start": 400000})
        self.assertNotIn("win11", json.dumps(activity))  # nome da VM nunca vai
        self.assert_discord_limits(activity)

    def test_vm_many_and_unknown(self):
        activity = self.build([vm_info(), vm_info(key="b", name="Gentoo", badge="gentoo")], None)
        self.assertEqual(activity["state"], "in 2 virtual machines")
        activity = self.build([vm_info(name=None, badge="generic")], None)
        self.assertEqual(activity["details"], "Orbiting a virtual machine")
        self.assertEqual(activity["assets"]["small_image"], "https://daeese.me/filearchive/presence/os/generic.png")
        activity = self.build([vm_info(badge="not-a-badge")], None)
        self.assertTrue(activity["assets"]["small_image"].endswith("/os/generic.png"))

    def test_vm_plus_coding(self):
        public = {"name": "Blos", "url": "https://github.com/o/Blos"}
        activity = self.build([vm_info()], coding_info(public=public, language="Luau", badge="luau"))
        self.assertEqual(activity["details"], "Orbiting Windows 11")
        self.assertEqual(activity["state"], "while writing Luau in Blos")
        self.assertEqual(activity["buttons"][1]["label"], "Blos on GitHub")
        activity = self.build([vm_info()], coding_info())
        self.assertEqual(activity["state"], "while writing Python in a private project")
        self.assert_discord_limits(activity)

    def test_config_overrides_and_bad_templates(self):
        config = make_config(coding={"details": "Plotting {nope}", "state": "Coding {language}"})
        snap = dp.select_state([], coding_info(public={"name": "r", "url": "https://github.com/o/r"}), dp.Timeline(), 5)
        activity = dp.build_activity(config, snap, 1.0)
        self.assertEqual(activity["details"], "Charting new stars")  # placeholder invalido -> padrao
        self.assertEqual(activity["state"], "Coding Python")
        with self.assertRaises(SystemExit):
            make_config(vm={"largeImage": "orbit"})

    def test_engine_stable_across_ticks(self):
        class FakeDetector:
            def __init__(self):
                self.coding = coding_info(public={"name": "r", "url": "https://github.com/o/r"})
                self.vms = []

            def warn_once(self, *_):
                pass

            def detect_vms(self, _procs):
                return self.vms

            def detect_coding(self, _procs):
                return self.coding

        clock = [1000.0]
        detector = FakeDetector()
        engine = dp.PresenceEngine(self.config, 1.0, detector=detector, clock=lambda: clock[0])
        first, _ = engine.compute()
        clock[0] += 15
        second, _ = engine.compute()
        self.assertEqual(dp.signature(first), dp.signature(second))
        detector.vms = [vm_info(started=900)]
        clock[0] += 15
        third, snap = engine.compute()
        self.assertEqual(snap["kind"], "vm")
        clock[0] += 15
        fourth, _ = engine.compute()
        self.assertEqual(dp.signature(third), dp.signature(fourth))
        # Atividade recusada pelo Discord cai para o idle.
        engine.rejected.add(dp.signature(fourth))
        clock[0] += 15
        fifth, _ = engine.compute()
        self.assertEqual(fifth, engine.idle_activity())


class GithubTest(unittest.TestCase):
    def setUp(self):
        self.clock = [1000.0]
        self.detector = dp.Detector(make_config(), clock=lambda: self.clock[0], home="/home/x")
        self.calls = 0

    def test_public_private_and_cache(self):
        def fetch(url):
            self.calls += 1
            self.assertEqual(url, "https://api.github.com/repos/o/R")
            return {"private": False, "name": "R", "html_url": "https://github.com/o/R"}

        self.assertEqual(self.detector.github_public("o", "R", fetch=fetch), {"name": "R", "url": "https://github.com/o/R"})
        self.detector.github_public("o", "R", fetch=fetch)
        self.assertEqual(self.calls, 1)
        self.clock[0] += dp.GITHUB_TTL + 1
        self.detector.github_public("o", "R", fetch=fetch)
        self.assertEqual(self.calls, 2)

        self.assertIsNone(self.detector.github_public("o", "p", fetch=lambda _: {"private": True, "name": "p"}))

    def silence_log(self):
        original = dp.log
        dp.log = lambda *_: None
        self.addCleanup(setattr, dp, "log", original)

    def test_errors_are_private(self):
        def not_found(url):
            raise urllib.error.HTTPError(url, 404, "Not Found", {}, io.BytesIO(b""))

        def offline(_url):
            raise OSError("network down")

        self.silence_log()
        self.assertIsNone(self.detector.github_public("o", "gone", fetch=not_found))
        self.assertIsNone(self.detector.github_public("o", "net", fetch=offline))
        # Erro de rede expira antes do "definitivo".
        self.assertLess(self.detector._github_cache[("o", "net")][0], self.detector._github_cache[("o", "gone")][0])


@unittest.skipUnless(shutil.which("git"), "git ausente")
class RecentChangesTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.repo = os.path.join(self.tmp, "proj")
        os.makedirs(self.repo)
        env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t",
                   GIT_COMMITTER_EMAIL="t@t", GIT_AUTHOR_DATE="2020-01-01T00:00:00",
                   GIT_COMMITTER_DATE="2020-01-01T00:00:00")
        run = lambda *a: subprocess.run(["git", "-C", self.repo, *a], check=True, env=env,
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        run("init", "-q")
        for name in ("main.rs", "lib.rs", "README.md"):
            with open(os.path.join(self.repo, name), "w") as handle:
                handle.write("x\n")
        run("add", ".")
        run("commit", "-q", "-m", "init")
        old = time.time() - 3 * 3600
        for name in ("main.rs", "lib.rs", "README.md"):
            os.utime(os.path.join(self.repo, name), (old, old))
        self.detector = dp.Detector(make_config(), home=self.tmp)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_editor_open_without_changes_is_not_coding(self):
        self.assertIsNone(self.detector.recent_changes(self.repo, time.time(), 1200))

    def test_recent_edit_counts(self):
        with open(os.path.join(self.repo, "main.rs"), "a") as handle:
            handle.write("y\n")
        with open(os.path.join(self.repo, "tool.py"), "w") as handle:
            handle.write("z\n")
        with open(os.path.join(self.repo, "helper.py"), "w") as handle:
            handle.write("z\n")
        change = self.detector.recent_changes(self.repo, time.time(), 1200)
        self.assertIsNotNone(change)
        self.assertEqual(dp.majority_language(change[1]), "python")
        self.assertEqual(self.detector.repo_language(self.repo, self.repo), "rust")

    def test_workspace_dir_expands_to_child_repos(self):
        repos = self.detector.candidate_repos({self.tmp, os.path.join(self.repo)})
        self.assertEqual(repos, {self.repo})
        self.assertEqual(dp.find_git_root(os.path.join(self.repo), self.tmp), self.repo)
        self.assertIsNone(dp.find_git_root(self.tmp, self.tmp))


class IpcFramingTest(unittest.TestCase):
    def setUp(self):
        self.a, self.b = socket.socketpair()
        self.a.settimeout(dp.SOCKET_TIMEOUT)

    def tearDown(self):
        self.a.close()
        self.b.close()

    def test_idle_then_frame(self):
        with self.assertRaises(dp.FrameIdle):
            dp.recv(self.a, idle_timeout=0.05)
        body = json.dumps({"nonce": "n"}).encode()
        self.b.sendall(struct.pack("<II", dp.OP_PING, len(body)) + body)
        self.assertEqual(dp.recv(self.a, idle_timeout=0.5), (dp.OP_PING, {"nonce": "n"}))
        self.assertEqual(self.a.gettimeout(), dp.SOCKET_TIMEOUT)

    def test_oversized_frame_drops_session(self):
        original = dp.log
        dp.log = lambda *_: None
        self.addCleanup(setattr, dp, "log", original)
        self.b.sendall(struct.pack("<II", dp.OP_FRAME, dp.MAX_FRAME_BYTES + 1))
        self.assertEqual(dp.recv(self.a, idle_timeout=0.5), (None, None))

    def test_eof(self):
        self.b.close()
        self.assertEqual(dp.recv(self.a, idle_timeout=0.5), (None, None))


if __name__ == "__main__":
    unittest.main()
