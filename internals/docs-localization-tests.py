#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Regression tests for documentation validation, including fail-closed fixtures."""

from copy import deepcopy
from pathlib import Path
import json
import runpy
from tempfile import TemporaryDirectory
import tomllib
import unittest
from unittest.mock import patch

from docs_html import Document, HtmlHeading, LocalTarget, local_target
from docs_sitemap import SITEMAP, check_sitemap, localized_sitemap
from docs_localization import (
    Locale,
    Page,
    Translation,
    check_config,
    check_translation,
    digest,
    markdown_surface,
    nav_structure,
    parse_manifest,
    shared_config,
)


class SourceTests(unittest.TestCase):
    def test_routes(self):
        self.assertEqual(Page.parse("index.md").url(Locale.EN), "/docs/")
        self.assertEqual(
            Page.parse("tutorial/index.md").url(Locale.JA), "/docs/ja/tutorial/"
        )
        self.assertEqual(
            Page.parse("language/type-system.md").url(Locale.EN),
            "/docs/language/type-system/",
        )

    def test_reject_ambiguous_or_escaping_paths(self):
        for path in [
            "../index.md",
            "/index.md",
            "a/../index.md",
            "a//index.md",
            "./index.md",
            "a\\index.md",
            "a.md#x",
            "https://example.com/a.md",
            "a.MD",
            "a/%2e%2e/b.md",
            2,
        ]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                Page.parse(path)

    def test_manifest_schema_is_closed(self):
        entry = Translation(
            Page.parse("index.md"), digest("en"), digest("ja")
        ).serialize()
        self.assertEqual(len(parse_manifest({"version": 1, "pages": [entry]})), 1)
        invalid = [
            {"version": True, "pages": [entry]},
            {"version": 2, "pages": [entry]},
            {"version": 1, "pages": [entry, entry]},
            {"version": 1, "pages": [{**entry, "status": "reviewed"}]},
            {"version": 1, "pages": [{**entry, "path": "../index.md"}]},
            {"version": 1, "pages": [{**entry, "english_sha256": "old"}]},
            {"version": 1, "pages": "index.md"},
        ]
        for value in invalid:
            with self.subTest(value=value), self.assertRaises(ValueError):
                parse_manifest(value)

    def test_fences_preserve_content_and_ignore_fake_headings(self):
        source = "# Title\n\n````markdown\n# not a heading\n```\n````\n\n## Next\n~~~gcl\nnode x: Int = 1;\n~~~\n"
        surface = markdown_surface(source)
        self.assertEqual([h.level for h in surface.headings], [1, 2])
        self.assertEqual(len(surface.examples), 2)
        self.assertIn("# not a heading", surface.examples[0])
        with self.assertRaisesRegex(ValueError, "Unclosed"):
            markdown_surface("```gcl\nnode x: Int = 1;\n")

    def test_translation_checks(self):
        page = Page.parse("index.md")
        en = "# Title\n\n```gcl\nnode x: Int = 1;\n```\n"
        ja = en.replace("Title", "タイトル { #title }")
        self.assertEqual(check_translation(page, en, ja), [])
        self.assertTrue(check_translation(page, en, ja.replace("= 1;", "= 2;")))
        self.assertTrue(
            check_translation(page, en, ja.replace("# タイトル", "## タイトル"))
        )
        self.assertTrue(check_translation(page, en, en))
        self.assertNotEqual(digest(en), digest(en + "\n"))

    def test_navigation_retains_nesting_and_order(self):
        en = [{"Home": "index.md"}, {"Tutorial": [{"Overview": "tutorial/index.md"}]}]
        ja = [
            {"ホーム": "index.md"},
            {"チュートリアル": [{"概要": "tutorial/index.md"}]},
        ]
        self.assertEqual(nav_structure(en), nav_structure(ja))
        self.assertNotEqual(nav_structure(en), nav_structure(ja[::-1]))
        with self.assertRaises(ValueError):
            nav_structure({"Home": "index.md", "Other": "other.md"})

    def test_configuration_drift_and_missing_pages(self):
        root = Path(__file__).resolve().parent.parent
        with (root / "zensical.toml").open("rb") as source:
            en = tomllib.load(source)["project"]
        with (root / "zensical.ja.toml").open("rb") as source:
            ja = tomllib.load(source)["project"]
        self.assertEqual(shared_config(en), shared_config(ja))
        broken = deepcopy(ja)
        broken["theme"]["features"].append("navigation.tabs")
        self.assertNotEqual(shared_config(en), shared_config(broken))
        pages = {
            Page.parse(path.relative_to(root / "docs/en").as_posix())
            for path in (root / "docs/en").rglob("*.md")
        }
        self.assertEqual(check_config(ja, Locale.JA, pages), [])
        self.assertTrue(check_config(ja, Locale.JA, set()))
        broken["site_dir"] = "site/docs"
        self.assertTrue(check_config(broken, Locale.JA, pages))


class HtmlTests(unittest.TestCase):
    def test_local_resolution(self):
        base = "/docs/ja/tutorial/step1/"
        self.assertEqual(
            local_target(base, "../step2/#次元"),
            LocalTarget("/docs/ja/tutorial/step2/", "次元"),
        )
        self.assertEqual(
            local_target(base, "https://graphcal.org/docs/assets/example.gcl"),
            LocalTarget("/docs/assets/example.gcl", ""),
        )
        self.assertIsNone(
            local_target(base, "https://github.com/graphcal-lang/graphcal")
        )
        self.assertIsNone(local_target(base, "mailto:example@example.com"))
        self.assertIsNone(local_target(base, "//other.example/test"))
        for target in ["/docs/%2e%2e/secret", "/docs/%5csecret", "/docs/%00secret"]:
            with self.subTest(target=target), self.assertRaises(ValueError):
                local_target(base, target)

    def test_html_boundary(self):
        document = Document("""<html lang="ja"><head>
<link rel="canonical" href="https://graphcal.org/docs/ja/">
<link rel="alternate" href="/docs/" hreflang="en"></head>
<h2 id="chrome">Not content</h2><article><h1 id="title">日本語</h1>
<p><code>Int &amp; Float</code><a href="#title">見出し</a></p>
<pre><code>do not count as inline</code></pre>
<table><tr><td>単位</td></tr></table><img src="/docs/assets/rocket.png"></article></html>""")
        self.assertEqual(document.language, "ja")
        self.assertEqual(document.headings, [HtmlHeading(1, "title")])
        self.assertEqual(document.inline_code, ["Int & Float"])
        self.assertEqual(document.table_rows, 1)
        self.assertEqual(document.identifiers, {"title", "chrome"})
        self.assertTrue(
            any(
                link.relation == "alternate" and link.language == "en"
                for link in document.links
            )
        )
        self.assertTrue(any(link.tag == "img" for link in document.links))


class FilesystemTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.commands = runpy.run_path(str(Path(__file__).with_name("check-docs.py")))

    def setUp(self):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.page = Page.parse("index.md")
        texts = {Locale.EN: "# Home\n", Locale.JA: "# ホーム { #home }\n"}
        for locale in Locale:
            source = self.root / "docs" / locale.value
            source.mkdir(parents=True)
            (source / "index.md").write_text(texts[locale], encoding="utf-8")
            filename = "zensical.toml" if locale == Locale.EN else "zensical.ja.toml"
            (self.root / filename).write_text(
                f'''[project]
site_name = "Test"
site_url = "https://graphcal.org{locale.prefix}"
docs_dir = "docs/{locale.value}"
site_dir = "target/docs-site/{locale.value}"
nav = [{{"Home" = "index.md"}}]
[project.theme]
language = "{locale.value}"
palette = []
[project.extra]
alternate = [{{name = "English", link = "/docs/", lang = "en"}}, {{name = "日本語", link = "/docs/ja/", lang = "ja"}}]
''',
                encoding="utf-8",
            )
        manifest = self.root / "manifest.json"
        manifest.write_text(
            json.dumps(
                {
                    "version": 1,
                    "pages": [
                        Translation(
                            self.page,
                            digest(texts[Locale.EN]),
                            digest(texts[Locale.JA]),
                        ).serialize()
                    ],
                }
            ),
            encoding="utf-8",
        )
        self.addCleanup(patch.stopall)
        patch.dict(
            self.commands["source_errors"].__globals__,
            {"ROOT": self.root, "MANIFEST": manifest},
        ).start()

    def test_missing_stale_and_orphaned_translations_fail(self):
        check = self.commands["source_errors"]
        self.assertEqual(check(), [])
        english = self.root / "docs/en/index.md"
        english.write_text("# Home\nChanged requirement.\n", encoding="utf-8")
        self.assertTrue(any("English changed" in error for error in check()))
        japanese = self.root / "docs/ja/index.md"
        japanese.write_text("# ホーム { #home }\n変更\n", encoding="utf-8")
        self.assertTrue(any("Japanese changed" in error for error in check()))
        japanese.unlink()
        with self.assertRaisesRegex(ValueError, "No documentation found"):
            check()
        (self.root / "docs/ja/orphan.md").write_text("# 孤立\n", encoding="utf-8")
        self.assertTrue(any("Japanese sources: missing" in error for error in check()))
        self.assertTrue(any("orphaned" in error for error in check()))

    def test_built_link_fragment_search_and_language_failures(self):
        site = self.root / "site"
        for locale in Locale:
            directory = site / locale.prefix.lstrip("/")
            directory.mkdir(parents=True, exist_ok=True)
            other = Locale.JA if locale == Locale.EN else Locale.EN
            (directory / "index.html").write_text(
                f'''<html lang="{locale.value}"><head><link rel="canonical" href="https://graphcal.org{locale.prefix}"></head><body><article><h1 id="home">Home</h1><a href="{other.prefix}" hreflang="{other.value}">Other language</a><a href="#home">Section</a></article></body></html>''',
                encoding="utf-8",
            )
            (directory / "404.html").write_text(
                f'<html lang="{locale.value}"><h1 id="__skip">404</h1></html>',
                encoding="utf-8",
            )
            xml = f'<urlset xmlns="{SITEMAP}"><url><loc>https://graphcal.org{locale.prefix}</loc></url></urlset>'
            (directory / "sitemap.xml").write_text(
                localized_sitemap(xml, locale, {self.page}), encoding="utf-8"
            )
            (directory / "search.json").write_text(
                json.dumps(
                    {
                        "config": {"lang": [locale.value]},
                        "items": [{"level": 1, "location": ""}],
                    }
                ),
                encoding="utf-8",
            )
        (site / "sitemap.xml").write_text(
            f'<sitemapindex xmlns="{SITEMAP}"><sitemap><loc>https://graphcal.org/docs/sitemap.xml</loc></sitemap><sitemap><loc>https://graphcal.org/docs/ja/sitemap.xml</loc></sitemap></sitemapindex>',
            encoding="utf-8",
        )
        check = self.commands["artifact_errors"]
        self.assertEqual(check(site), [])
        path = site / "docs/ja/index.html"
        path.write_text(
            path.read_text()
            .replace('lang="ja"', 'lang="en"')
            .replace('href="#home"', 'href="#missing"')
            + '<img src="/missing.png">',
            encoding="utf-8",
        )
        errors = check(site)
        self.assertTrue(any("incorrect HTML language" in error for error in errors))
        self.assertTrue(any("missing fragment" in error for error in errors))
        self.assertTrue(any("missing local resource" in error for error in errors))
        (site / "docs/ja/search.json").write_text(
            json.dumps(
                {
                    "config": {"lang": ["en"]},
                    "items": [{"level": 1, "location": "/docs/"}],
                }
            )
        )
        errors = check(site)
        self.assertTrue(any("incorrect search language" in error for error in errors))
        self.assertTrue(any("search index includes" in error for error in errors))


class SitemapTests(unittest.TestCase):
    def test_reciprocal_alternates_and_idempotence(self):
        pages = {Page.parse("index.md"), Page.parse("tutorial/index.md")}
        for locale in Locale:
            xml = (
                f'<urlset xmlns="{SITEMAP}">'
                + "".join(
                    f"<url><loc>https://graphcal.org{page.url(locale)}</loc></url>"
                    for page in sorted(pages)
                )
                + "</urlset>"
            )
            result = localized_sitemap(xml, locale, pages)
            self.assertEqual(check_sitemap(result, locale, pages), [])
            self.assertEqual(localized_sitemap(result, locale, pages), result)
            self.assertTrue(
                check_sitemap(
                    result.replace('hreflang="ja"', 'hreflang="en"'), locale, pages
                )
            )
            with self.assertRaisesRegex(ValueError, "incomplete or stale"):
                localized_sitemap(xml, locale, pages | {Page.parse("extra.md")})
            with self.assertRaises(ValueError):
                localized_sitemap(
                    xml.replace(
                        "</urlset>",
                        "<url><loc>https://graphcal.org/foreign/</loc></url></urlset>",
                    ),
                    locale,
                    pages,
                )


if __name__ == "__main__":
    unittest.main()
