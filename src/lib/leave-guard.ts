/** Where a guarded navigation is heading. `other` always leaves the resolver. */
export type LeaveTarget =
  | { kind: "selectRoot"; slug: string }
  | { kind: "openResolver"; slug: string; rel: string }
  | { kind: "other" };

export interface LeaveState {
  selectedSlug: string | null;
  resolvingRel: string | null;
}

/** True when the navigation keeps the open resolver as it is. */
export function isSameTarget(state: LeaveState, target: LeaveTarget): boolean {
  switch (target.kind) {
    case "selectRoot":
      return state.selectedSlug === target.slug;
    case "openResolver":
      return (
        state.selectedSlug === target.slug && state.resolvingRel === target.rel
      );
    case "other":
      return false;
  }
}

/** Ask before a navigation closes a resolver with unsaved Result changes. */
export function shouldPromptLeave(
  state: LeaveState,
  dirty: boolean,
  target: LeaveTarget,
): boolean {
  if (state.resolvingRel === null || !dirty) return false;
  return !isSameTarget(state, target);
}

/** Clear the dirty flag before closing, so a finished resolve never asks. */
export function closeClean(
  setDirty: (dirty: boolean) => void,
  close: () => void,
): void {
  setDirty(false);
  close();
}
