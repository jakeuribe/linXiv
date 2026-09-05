import { libraryFetch } from "../stores/backend.ts";
import type { Author, AuthorDetail, BasicAuthorDetails } from "../types/api";

export interface AuthorUpdateBody {
  full_name?: string | null;
  first_name?: string | null;
  last_name?: string | null;
  orcid?: string | null;
}

export async function listAuthors(excludeSingle = false): Promise<Author[]> {
  const query = excludeSingle ? "?exclude_single=true" : "";
  const data = await libraryFetch<{ authors: Author[] }>(`/api/authors${query}`);
  return data.authors;
}

export async function getAuthor(authorId: number): Promise<AuthorDetail> {
  return libraryFetch<AuthorDetail>(`/api/authors/${authorId}`);
}

export async function updateAuthor(
  authorId: number,
  body: AuthorUpdateBody,
): Promise<AuthorDetail> {
  return libraryFetch<AuthorDetail>(`/api/authors/${authorId}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

// Envelope assembled inline by route/authors.rs — no core struct to generate.
export interface MergeCandidates {
  /** Shares this author's ORCID — near-certain duplicate. */
  candidates: BasicAuthorDetails[];
  /** Shares only the exact full name — weak evidence, never overlaps `candidates`. */
  name_candidates: BasicAuthorDetails[];
}

export async function getMergeCandidates(authorId: number): Promise<MergeCandidates> {
  return libraryFetch<MergeCandidates>(`/api/authors/${authorId}/merge-candidates`);
}

export async function linkAuthorToPaper(authorId: number, paperId: number): Promise<void> {
  await libraryFetch<{ ok: boolean }>(`/api/authors/${authorId}/papers/${paperId}`, {
    method: "POST",
  });
}

export async function unlinkAuthorFromPaper(authorId: number, paperId: number): Promise<void> {
  await libraryFetch<{ ok: boolean }>(`/api/authors/${authorId}/papers/${paperId}`, {
    method: "DELETE",
  });
}

export async function mergeAuthors(
  canonicalId: number,
  duplicateIds: number[],
): Promise<AuthorDetail> {
  return libraryFetch<AuthorDetail>(`/api/authors/${canonicalId}/merge`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ duplicate_ids: duplicateIds }),
  });
}

export async function deleteAuthor(authorId: number): Promise<void> {
  await libraryFetch<{ ok: boolean }>(`/api/authors/${authorId}`, {
    method: "DELETE",
  });
}
