import os
from dataclasses import dataclass
from typing import cast
from google import genai
from google.genai import types
from pydantic import BaseModel
from dotenv import load_dotenv

_ = load_dotenv()
_client: genai.Client | None = None


def _get_client() -> genai.Client:
    global _client
    if _client is None:
        api_key = os.getenv("GENAI_API_KEY_TAG_GEN")
        if not api_key:
            raise EnvironmentError("GENAI_API_KEY_TAG_GEN not set.")
        _client = genai.Client(api_key=api_key)
    return _client


@dataclass
class PaperContent:
    """
    Wraps available content for a paper.
    Functions use the richest source available: pdf > full_text > abstract.
    Populate full_text (TeX) or pdf (bytes) via downloads.py when needed.
    """
    abstract: str
    full_text: str | None = None  # raw TeX source
    pdf: bytes | None = None      # PDF bytes

    def to_parts(self) -> list[types.Part]:
        if self.pdf is not None:
            return [types.Part.from_bytes(data=self.pdf, mime_type="application/pdf")]
        if self.full_text is not None:
            return [types.Part.from_text(text=self.full_text)]
        return [types.Part.from_text(text=self.abstract)]


def _generate(prompt: str, content: PaperContent, schema: type[BaseModel]) -> BaseModel:
    parts = [types.Part.from_text(text=prompt)] + content.to_parts()
    response = _get_client().models.generate_content(
        model="gemini-2.0-flash",
        contents=parts,
        config=types.GenerateContentConfig(
            response_mime_type="application/json",
            response_schema=schema,
        ),
    )
    return cast(BaseModel, response.parsed)


# Response schemas

class _TagResponse(BaseModel):
    tags: list[str]


class _SummaryResponse(BaseModel):
    tldr: str
    key_contributions: list[str]


class _RelatedResponse(BaseModel):
    related_ids: list[str]

# Public API


def tag(content: PaperContent, file_path: str | None = None) -> list[str]:
    """Generate 3-5 Obsidian tags. Optionally append to file_path."""
    parsed = cast(_TagResponse, _generate(
        "Generate 3-5 relevant Obsidian tags for this academic paper.",
        content, _TagResponse,
    ))
    tags = [f"#{t.strip().lstrip('#').replace(' ', '_')}" for t in parsed.tags]
    if file_path is not None:
        with open(file_path, "a", encoding="utf-8") as f:
            f.write("\n" + " ".join(tags))
    return tags


def summarize(content: PaperContent) -> _SummaryResponse:
    """Return a one-sentence TLDR and 2-4 key contributions."""
    return cast(_SummaryResponse, _generate(
        "Summarize this academic paper into a one-sentence TLDR and 2-4 key contributions.",
        content, _SummaryResponse,
    ))


def find_related(
    content: PaperContent,
    candidates: list[tuple[str, str]],   # [(paper_id, abstract), ...]
    threshold: int = 5,
) -> list[str]:
    """
    Return IDs of the most conceptually related papers from candidates.
    Useful for adding semantic edges to the graph beyond shared category/author.
    """
    candidate_block = "\n\n".join(
        f"ID: {pid}\n{ab}" for pid, ab in candidates[:40])
    parsed = cast(_RelatedResponse, _generate(
        f"Which of the following papers are most conceptually related to this one? "
        f"Return up to {threshold} paper IDs.\n\n{candidate_block}",
        content, _RelatedResponse,
    ))
    return parsed.related_ids
