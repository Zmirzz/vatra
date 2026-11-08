import {create} from "zustand";

/** A page the person was sent to from somewhere else, such as the report a scheduled research left. */
export const useHubFocus = create<{researchId: number | null}>(() => ({researchId: null}));
