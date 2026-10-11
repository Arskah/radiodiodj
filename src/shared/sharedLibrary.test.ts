import { describe, expect, it } from "vitest";
import {
  needsAttention,
  replacesLibrary,
  roleLabel,
  waitingLabel,
} from "./sharedLibrary";

describe("replacesLibrary", () => {
  it("is true only for a machine becoming a studio", () => {
    expect(replacesLibrary("studio", "standalone", "standalone")).toBe(true);
    expect(replacesLibrary("studio", "owner", "owner")).toBe(true);
  });

  it("is false for a machine that is one already", () => {
    expect(replacesLibrary("studio", "studio", "studio")).toBe(false);
  });

  it("is false once the join is saved and waiting for a restart", () => {
    expect(replacesLibrary("studio", "standalone", "studio")).toBe(false);
  });

  it("is false for every other role", () => {
    expect(replacesLibrary("owner", "standalone", "standalone")).toBe(false);
    expect(replacesLibrary("standalone", "studio", "studio")).toBe(false);
  });
});

describe("roleLabel", () => {
  it("names each role", () => {
    expect(roleLabel("standalone")).toBe("Not shared");
    expect(roleLabel("owner")).toBe("Library owner");
    expect(roleLabel("studio")).toBe("Studio");
  });
});

describe("needsAttention", () => {
  const status = {
    role: "studio" as const,
    ok: false,
    message: "",
    reachedAt: null,
    encrypted: null,
    waiting: 0,
  };

  it("is quiet for a computer that shares nothing", () => {
    expect(
      needsAttention({ ...status, role: "standalone", message: "x" }),
    ).toBe(false);
  });

  it("is quiet until the first visit has something to say", () => {
    expect(needsAttention({ ...status, role: "studio" })).toBe(false);
  });

  it("is raised by a hub that cannot be reached", () => {
    expect(
      needsAttention({ ...status, role: "owner", message: "No route." }),
    ).toBe(true);
  });

  it("is quiet when the visit went as the role asks", () => {
    expect(
      needsAttention({ ...status, role: "studio", ok: true, message: "Fine." }),
    ).toBe(false);
  });
});

describe("waitingLabel", () => {
  const status = {
    role: "studio" as const,
    ok: true,
    message: "Fine.",
    reachedAt: 5,
    encrypted: true,
    waiting: 0,
  };

  it("says nothing when nothing is waiting", () => {
    expect(waitingLabel(status)).toBe("");
    expect(waitingLabel({ ...status, role: "standalone", waiting: 3 })).toBe(
      "",
    );
  });

  it("counts what is on its way", () => {
    expect(waitingLabel({ ...status, waiting: 1 })).toBe(
      "1 change is on the way to the hub.",
    );
    expect(waitingLabel({ ...status, waiting: 4 })).toBe(
      "4 changes are on the way to the hub.",
    );
  });

  it("says they are held while the hub is away", () => {
    expect(waitingLabel({ ...status, ok: false, waiting: 2 })).toBe(
      "2 changes are waiting here until the hub can be reached.",
    );
  });
});
