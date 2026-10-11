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
