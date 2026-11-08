export function pickerPlacement(anchor: { top: number; bottom: number; left: number; right: number }, viewport: { width: number; height: number }, align: "start" | "end") {
  const margin = 12;
  const gap = 8;
  const width = Math.min(400, Math.max(0, viewport.width - margin * 2));
  const above = Math.max(0, anchor.top - gap - margin);
  const below = Math.max(0, viewport.height - anchor.bottom - gap - margin);
  const opensAbove = above >= 400 || above > below;
  const left = Math.max(margin, Math.min(align === "start" ? anchor.left : anchor.right - width, viewport.width - width - margin));
  return {
    width, left,
    maxHeight: Math.min(440, opensAbove ? above : below),
    ...(opensAbove ? { bottom: Math.max(margin, viewport.height - anchor.top + gap) } : { top: Math.max(margin, anchor.bottom + gap) }),
  };
}
