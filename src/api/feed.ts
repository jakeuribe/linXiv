import { libraryFetch } from "../stores/backend.ts";
import type {
  FeedDismissBody,
  FeedFilterRule,
  FeedResponse,
  FeedRuleCreateBody,
  FeedRulesResponse,
} from "../types/api";

export async function getFeed(url: string): Promise<FeedResponse> {
  return libraryFetch<FeedResponse>(`/api/feed?url=${encodeURIComponent(url)}`);
}

export async function dismissFeedEntry(
  arxivId: string,
  version: number,
  permanent = false,
): Promise<void> {
  const body: FeedDismissBody = { arxiv_id: arxivId, version, permanent };
  await libraryFetch("/api/feed/dismiss", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function listFeedRules(): Promise<FeedFilterRule[]> {
  const res = await libraryFetch<FeedRulesResponse>("/api/feed/rules");
  return res.rules;
}

export async function createFeedRule(
  field: FeedFilterRule["field"],
  keywords: string,
  action: FeedFilterRule["action"] = "DENY",
): Promise<void> {
  const body: FeedRuleCreateBody = { field, keywords, action };
  await libraryFetch("/api/feed/rules", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function deleteFeedRule(ruleId: number): Promise<void> {
  await libraryFetch(`/api/feed/rules/${ruleId}`, { method: "DELETE" });
}
