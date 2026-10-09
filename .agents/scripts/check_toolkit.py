"""Check contributor documents: local links and anchors, toolkit metadata, catalog and issue forms."""

import re
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit

try:
    import yaml
    from markdown_it import MarkdownIt
except ImportError as error:
    sys.exit(f"{error}; install .agents/scripts/requirements.txt to run the toolkit checks")


ROOT = Path(__file__).resolve().parents[2]
MARKDOWN = MarkdownIt("commonmark")
FRONTMATTER = re.compile(r"\A---\n(.*?)\n---(?:\n|\Z)", re.DOTALL)
CATALOGUED = ("commands", "agents", "skills")
TOP_LEVEL = ("AGENTS.md", "CONTRIBUTING.md", "README.md", "BLUEPRINT.md",
             ".github/pull_request_template.md")
SLUG_DROP = re.compile(r"[^\w\- ]", re.UNICODE)


def body(source):
    match = FRONTMATTER.match(source)
    return source[match.end():] if match else source


def tokens(source):
    for block in MARKDOWN.parse(body(source)):
        yield block
        yield from block.children or []


def local_links(path, source, *, images=True):
    """Yield (target, resolved path, fragment) for each relative link or image."""
    for token in tokens(source):
        if token.type == "link_open":
            target = token.attrGet("href")
        elif images and token.type == "image":
            target = token.attrGet("src")
        else:
            continue
        if not target:
            continue
        url = urlsplit(target)
        if url.scheme or url.netloc:
            continue
        resolved = (path.parent / unquote(url.path)).resolve() if url.path else path.resolve()
        yield target, resolved, unquote(url.fragment)


def anchors(source):
    """Return GitHub-style heading anchors, including numbered duplicates."""
    result, seen = set(), {}
    blocks = MARKDOWN.parse(body(source))
    for index, block in enumerate(blocks):
        if block.type != "heading_open":
            continue
        inline = blocks[index + 1]
        text = "".join(child.content for child in inline.children or []
                       if child.type in ("text", "code_inline"))
        slug = SLUG_DROP.sub("", text.strip().lower()).replace(" ", "-")
        count = seen.get(slug, 0)
        seen[slug] = count + 1
        result.add(slug if count == 0 else f"{slug}-{count}")
    return result


def metadata_fields(source):
    """Return name/description from a YAML mapping with unique string fields, else None."""
    try:
        node = yaml.compose(source, Loader=yaml.SafeLoader)
    except yaml.YAMLError:
        return None
    if not isinstance(node, yaml.MappingNode):
        return None
    fields = {}
    for key, value in node.value:
        if (not isinstance(key, yaml.ScalarNode) or key.tag != "tag:yaml.org,2002:str"
                or key.value in fields):
            return None
        fields[key.value] = value
    result = {}
    for name in ("name", "description"):
        value = fields.get(name)
        if not isinstance(value, yaml.ScalarNode) or value.tag != "tag:yaml.org,2002:str":
            return None
        result[name] = value.value
    return result


def check_issue_forms(root):
    errors = []
    for path in sorted((root / ".github/ISSUE_TEMPLATE").glob("*.yml")):
        label = path.relative_to(root)
        try:
            form = yaml.safe_load(path.read_text(encoding="utf-8"))
        except yaml.YAMLError as error:
            errors.append(f"{label}: invalid YAML: {error}")
            continue
        if path.name == "config.yml":
            if not isinstance(form, dict) or "blank_issues_enabled" not in form:
                errors.append(f"{label}: missing blank_issues_enabled")
            continue
        if (not isinstance(form, dict) or not isinstance(form.get("name"), str)
                or not isinstance(form.get("description"), str)
                or not isinstance(form.get("body"), list) or not form["body"]):
            errors.append(f"{label}: issue form needs name, description and a nonempty body")
            continue
        ids = [item.get("id") for item in form["body"]
               if isinstance(item, dict) and item.get("type") != "markdown"]
        if None in ids or len(ids) != len(set(ids)):
            errors.append(f"{label}: every input needs a unique id")
    return errors


def check(root):
    root = root.resolve()
    toolkit = root / ".agents"
    catalog_path = toolkit / "README.md"
    if not catalog_path.is_file():
        return ["missing .agents/README.md"]
    catalog = catalog_path.read_text(encoding="utf-8")
    catalog_targets = {resolved for _, resolved, _ in local_links(catalog_path, catalog,
                                                                   images=False)}
    errors = []
    documents = sorted(toolkit.rglob("*.md")) + [root / name for name in TOP_LEVEL]
    anchor_cache = {}
    for path in documents:
        label = path.relative_to(root)
        if not path.is_file():
            errors.append(f"missing {label}")
            continue
        source = path.read_text(encoding="utf-8")
        for target, resolved, fragment in local_links(path, source):
            if not resolved.is_relative_to(root) or not resolved.exists():
                errors.append(f"{label}: broken local link {target}")
                continue
            if fragment and resolved.suffix == ".md":
                if resolved not in anchor_cache:
                    anchor_cache[resolved] = anchors(resolved.read_text(encoding="utf-8"))
                if fragment not in anchor_cache[resolved]:
                    errors.append(f"{label}: missing anchor {target}")
        relative = path.relative_to(toolkit) if path.is_relative_to(toolkit) else None
        if relative and relative.parts[0] in CATALOGUED:
            match = FRONTMATTER.match(source)
            if not match:
                errors.append(f"{label}: missing front matter")
                continue
            fields = metadata_fields(match[1])
            expected = path.parent.name if path.name == "SKILL.md" else path.stem
            if not fields or fields["name"] != expected or not fields["description"].strip():
                errors.append(f"{label}: name must match path and description must be nonempty; "
                              "use a YAML mapping with unique string fields")
            if path.resolve() not in catalog_targets:
                errors.append(f"{label}: absent from toolkit catalog")
    for kind in CATALOGUED:
        if not list((toolkit / kind).rglob("*.md")):
            errors.append(f"no documents in .agents/{kind}")
    return errors + check_issue_forms(root)


def main():
    errors = check(ROOT)
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("Contributor toolkit links, anchors, metadata, catalog and issue forms passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
