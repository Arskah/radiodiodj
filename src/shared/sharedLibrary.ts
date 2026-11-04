import { formatAgo } from "./health";
import type { HubStatus, HubTakeOver, LibraryRole } from "./types";

/** Why an owner control is disabled on a studio. Shown as its tooltip. */
export const OWNER_ONLY =
  "The library owner does this. This computer is a studio of a shared library.";

/**
 * Whether the shared library wants an operator's eye: the hub cannot be
 * reached, or this computer cannot do what its role asks. A role that has not
 * visited the hub yet has nothing to say either way.
 */
export function needsAttention(status: HubStatus): boolean {
  return status.role !== "standalone" && !status.ok && status.message !== "";
}

/**
 * What is still to go to the hub, as the Settings page says it. Empty when
 * nothing is: an outbox that is empty is not news.
 */
export function waitingLabel(status: HubStatus): string {
  if (status.role === "standalone" || status.waiting === 0) return "";
  const what =
    status.waiting === 1 ? "1 change is" : `${status.waiting} changes are`;
  if (status.remedy?.kind === "joinAgain") {
    return `${what} saved here and cannot be sent.`;
  }
  // While the hub is away they wait for it; otherwise they are on their way.
  return status.ok
    ? `${what} on the way to the hub.`
    : `${what} waiting here until the hub can be reached.`;
}

/** A role as the Settings page names it. */
export function roleLabel(role: LibraryRole): string {
  switch (role) {
    case "standalone":
      return "Not shared";
    case "owner":
      return "Library owner";
    case "studio":
      return "Studio";
  }
}

/**
 * Whether saving `next` would replace this computer's library at the next
 * launch: a machine becomes a studio by setting its own library aside. One
 * that is a studio already, or is already saved as one, has been through it.
 */
export function replacesLibrary(
  next: LibraryRole,
  running: LibraryRole,
  saved: LibraryRole,
): boolean {
  return next === "studio" && running !== "studio" && saved !== "studio";
}

/**
 * What joining again costs in work this studio saved and never sent, for the
 * confirmation. Empty when nothing is waiting.
 */
export function lostByJoiningAgain(waiting: number): string {
  if (waiting === 0) return "";
  const what =
    waiting === 1
      ? "1 change saved here was never sent and is"
      : `${waiting} changes saved here were never sent and are`;
  return `${what} lost with it.`;
}

/** When the computer holding the owner role last visited the hub. */
export function ownerLastSeen(
  remedy: Pick<HubTakeOver, "owner" | "quietFor">,
  now = Date.now(),
): string {
  return remedy.quietFor === null
    ? `${remedy.owner} has never visited the hub.`
    : `${remedy.owner} last visited the hub ${formatAgo(now - remedy.quietFor * 1000, now)}.`;
}

/**
 * What taking the owner role does to the hub and to the studios, for the
 * confirmation: nothing where the hub holds this computer's library, and
 * everything where it holds another.
 */
export function takeOverEffect(
  remedy: Pick<HubTakeOver, "sameLibrary">,
): string {
  return remedy.sameLibrary
    ? "The hub holds this computer's library, so it keeps what it has and the studios are not affected."
    : "The hub holds a different library than this computer's. It is emptied and this library is published in its place, and every studio has to join again.";
}
