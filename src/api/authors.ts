import { apiFetch } from "./client";
import type { Author, AuthorDetail } from "../types/api";

export interface AuthorUpdateBody {
  full_name?: string | null;
  first_name?: string | null;
  last_name?: string | null;
  orcid?: string | null;
}

export async function listAuthors(excludeSingle = false): Promise<Author[]> {
  const query = excludeSingle ? "?exclude_single=true" : "";
  const data = await apiFetch<{ authors: Author[] }>(`/api/authors${query}`);
  return data.authors;
}

export async function getAuthor(authorId: number): Promise<AuthorDetail> {
  return apiFetch<AuthorDetail>(`/api/authors/${authorId}`);
}

export async function updateAuthor(
  authorId: number,
  body: AuthorUpdateBody,
): Promise<AuthorDetail> {
  return apiFetch<AuthorDetail>(`/api/authors/${authorId}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

export async function mergeAuthors(
  canonicalId: number,
  duplicateIds: number[],
): Promise<AuthorDetail> {
  return apiFetch<AuthorDetail>(`/api/authors/${canonicalId}/merge`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ duplicate_ids: duplicateIds }),
  });
}

export async function deleteAuthor(authorId: number): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/authors/${authorId}`, {
    method: "DELETE",
  });
}
