import type { HubStatus, LibraryRole } from "./types";

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
