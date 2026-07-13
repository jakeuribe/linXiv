import type { MemberRole, SharedSummary } from "../api/share";
import type { Project } from "../types/api";

/** The reader's own capability on the received share backing `project`, or
 * undefined when the project isn't linked to a received share or the role is
 * unknown (node offline, plain mirror, hosted project). Unknown → editable:
 * the write boundary is enforced server+crypto side (spec §7); this only
 * decides which affordances render. */
export function receivedShareRole(
  project: Pick<Project, "share_id"> | undefined,
  received: SharedSummary[] | undefined
): MemberRole | undefined {
  if (!project?.share_id) return undefined;
  return received?.find((s) => s.share_id === project.share_id)?.role;
}
