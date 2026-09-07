import { libraryFetch } from "../stores/backend.ts";
import type {
  Author,
  AuthorDetail,
  AuthorMergeResponse,
  AuthorsResponse,
  AuthorUpdateBody,
  MergeCandidates,
  OkReceipt,
} from "../types/api";

export type { AuthorUpdateBody };

export async function listAuthors(excludeSingle = false): Promise<Author[]> {
  const query = excludeSingle ? "?exclude_single=true" : "";
  const data = await libraryFetch<AuthorsResponse>(`/api/authors${query}`);
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

export type { MergeCandidates };

export async function getMergeCandidates(authorId: number): Promise<MergeCandidates> {
  return libraryFetch<MergeCandidates>(`/api/authors/${authorId}/merge-candidates`);
}

export async function linkAuthorToPaper(authorId: number, paperId: number): Promise<void> {
  await libraryFetch<OkReceipt>(`/api/authors/${authorId}/papers/${paperId}`, {
    method: "POST",
  });
}

export async function unlinkAuthorFromPaper(authorId: number, paperId: number): Promise<void> {
  await libraryFetch<OkReceipt>(`/api/authors/${authorId}/papers/${paperId}`, {
    method: "DELETE",
  });
}

export async function mergeAuthors(
  canonicalId: number,
  duplicateIds: number[],
): Promise<AuthorMergeResponse> {
  return libraryFetch<AuthorMergeResponse>(`/api/authors/${canonicalId}/merge`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ duplicate_ids: duplicateIds }),
  });
}

export async function deleteAuthor(authorId: number): Promise<void> {
  await libraryFetch<OkReceipt>(`/api/authors/${authorId}`, {
    method: "DELETE",
  });
}
