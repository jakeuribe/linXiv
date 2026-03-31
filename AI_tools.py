import os
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import cast
from pydantic import BaseModel
from dotenv import load_dotenv

_ = load_dotenv()


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

    def best_text(self) -> str:
        if self.full_text is not None:
            return self.full_text
        return self.abstract


# Response schemas

class _TagResponse(BaseModel):
    tags: list[str]


class SummaryResult(BaseModel):
    tldr: str
    key_contributions: list[str]


class _RelatedResponse(BaseModel):
    related_ids: list[str]


# ---------------------------------------------------------------------------
# AIProvider ABC
# ---------------------------------------------------------------------------

class AIProvider(ABC):
    """Unified interface for AI-powered paper analysis."""

    @abstractmethod
    def tag(self, content: PaperContent) -> list[str]:
        """Generate 3-5 relevant tags for a paper."""

    @abstractmethod
    def summarize(self, content: PaperContent) -> SummaryResult:
        """Return a one-sentence TLDR and 2-4 key contributions."""

    @abstractmethod
    def find_related(
        self,
        content: PaperContent,
        candidates: list[tuple[str, str]],
        threshold: int = 5,
    ) -> list[str]:
        """Return IDs of the most conceptually related papers from candidates."""


# ---------------------------------------------------------------------------
# GeminiProvider
# ---------------------------------------------------------------------------

class GeminiProvider(AIProvider):
    """AI provider backed by Google Gemini."""

    def __init__(self) -> None:
        self._client = None  # lazy init

    def _get_client(self):
        if self._client is None:
            from google import genai  # type: ignore[reportMissingImports]
            api_key = os.getenv("GENAI_API_KEY_TAG_GEN")
            if not api_key:
                raise EnvironmentError("GENAI_API_KEY_TAG_GEN not set.")
            self._client = genai.Client(api_key=api_key)
        return self._client

    def _generate(self, prompt: str, content: PaperContent, schema: type[BaseModel]) -> BaseModel:
        from google.genai import types  # type: ignore[reportMissingImports]
        parts: list = [types.Part.from_text(text=prompt)]
        if content.pdf is not None:
            parts.append(types.Part.from_bytes(data=content.pdf, mime_type="application/pdf"))
        elif content.full_text is not None:
            parts.append(types.Part.from_text(text=content.full_text))
        else:
            parts.append(types.Part.from_text(text=content.abstract))
        response = self._get_client().models.generate_content(
            model="gemini-2.0-flash",
            contents=parts,
            config=types.GenerateContentConfig(
                response_mime_type="application/json",
                response_schema=schema,
            ),
        )
        return cast(BaseModel, response.parsed)

    def tag(self, content: PaperContent) -> list[str]:
        parsed = cast(_TagResponse, self._generate(
            "Generate 3-5 relevant Obsidian tags for this academic paper.",
            content, _TagResponse,
        ))
        return [f"#{t.strip().lstrip('#').replace(' ', '_')}" for t in parsed.tags]

    def summarize(self, content: PaperContent) -> SummaryResult:
        return cast(SummaryResult, self._generate(
            "Summarize this academic paper into a one-sentence TLDR and 2-4 key contributions.",
            content, SummaryResult,
        ))

    def find_related(
        self,
        content: PaperContent,
        candidates: list[tuple[str, str]],
        threshold: int = 5,
    ) -> list[str]:
        candidate_block = "\n\n".join(
            f"ID: {pid}\n{ab}" for pid, ab in candidates[:40])
        parsed = cast(_RelatedResponse, self._generate(
            f"Which of the following papers are most conceptually related to this one? "
            f"Return up to {threshold} paper IDs.\n\n{candidate_block}",
            content, _RelatedResponse,
        ))
        return parsed.related_ids


# ---------------------------------------------------------------------------
# OpenAIProvider
# ---------------------------------------------------------------------------

class OpenAIProvider(AIProvider):
    """AI provider backed by OpenAI (GPT-4o, etc.)."""

    def __init__(self, model: str = "gpt-4o-mini") -> None:
        self._client = None  # lazy init
        self._model = model

    def _get_client(self):
        if self._client is None:
            from openai import OpenAI  # type: ignore[reportMissingImports]
            api_key = os.getenv("OPENAI_API_KEY")
            if not api_key:
                raise EnvironmentError("OPENAI_API_KEY not set.")
            self._client = OpenAI(api_key=api_key)
        return self._client

    def _generate(self, prompt: str, content: PaperContent, schema: type[BaseModel]) -> BaseModel:
        text = content.best_text()
        response = self._get_client().responses.parse(
            model=self._model,
            instructions=prompt,
            input=[{"role": "user", "content": text}],
            text_format=schema,
        )
        return response.output_parsed  # type: ignore[return-value]

    def tag(self, content: PaperContent) -> list[str]:
        parsed = cast(_TagResponse, self._generate(
            "Generate 3-5 relevant Obsidian tags for this academic paper. "
            "Return as JSON with a 'tags' array of strings.",
            content, _TagResponse,
        ))
        return [f"#{t.strip().lstrip('#').replace(' ', '_')}" for t in parsed.tags]

    def summarize(self, content: PaperContent) -> SummaryResult:
        return cast(SummaryResult, self._generate(
            "Summarize this academic paper into a one-sentence TLDR and 2-4 key contributions. "
            "Return as JSON with 'tldr' (string) and 'key_contributions' (array of strings).",
            content, SummaryResult,
        ))

    def find_related(
        self,
        content: PaperContent,
        candidates: list[tuple[str, str]],
        threshold: int = 5,
    ) -> list[str]:
        candidate_block = "\n\n".join(
            f"ID: {pid}\n{ab}" for pid, ab in candidates[:40])
        parsed = cast(_RelatedResponse, self._generate(
            f"Which of the following papers are most conceptually related to this one? "
            f"Return up to {threshold} paper IDs as JSON with a 'related_ids' array.\n\n{candidate_block}",
            content, _RelatedResponse,
        ))
        return parsed.related_ids


# ---------------------------------------------------------------------------
# Module-level active provider + public API
# ---------------------------------------------------------------------------

_provider: AIProvider | None = None


def _get_provider() -> AIProvider:
    global _provider
    if _provider is None:
        _provider = GeminiProvider()
    return _provider


def set_provider(provider: AIProvider) -> None:
    """Switch the active AI provider."""
    global _provider
    _provider = provider


def tag(content: PaperContent, file_path: str | None = None) -> list[str]:
    """Generate 3-5 Obsidian tags. Optionally append to file_path."""
    tags = _get_provider().tag(content)
    if file_path is not None:
        with open(file_path, "a", encoding="utf-8") as f:
            f.write("\n" + " ".join(tags))
    return tags


def summarize(content: PaperContent) -> SummaryResult:
    """Return a one-sentence TLDR and 2-4 key contributions."""
    return _get_provider().summarize(content)


def find_related(
    content: PaperContent,
    candidates: list[tuple[str, str]],   # [(paper_id, abstract), ...]
    threshold: int = 5,
) -> list[str]:
    """
    Return IDs of the most conceptually related papers from candidates.
    Useful for adding semantic edges to the graph beyond shared category/author.
    """
    return _get_provider().find_related(content, candidates, threshold)
