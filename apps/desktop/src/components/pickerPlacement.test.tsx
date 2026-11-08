import { render } from "@testing-library/react";
import { expect, test } from "vitest";
import { ProviderIcon } from "./ProviderIcon";
import { pickerPlacement } from "./pickerPlacement";

test("a narrow viewport keeps the picker below its anchor and within its edges", () => {
  expect(pickerPlacement({ top: 180, bottom: 210, left: 240, right: 300 }, { width: 320, height: 600 }, "end"))
    .toEqual({ width: 296, left: 12, top: 218, maxHeight: 370 });
});

test("a low anchor opens upwards and clamps horizontal placement", () => {
  expect(pickerPlacement({ top: 700, bottom: 730, left: 500, right: 650 }, { width: 800, height: 800 }, "start"))
    .toEqual({ width: 400, left: 388, bottom: 108, maxHeight: 440 });
});

test("provider masks quote bundled SVG URLs", () => {
  const { container } = render(<ProviderIcon provider="claude" />);
  const mask = (container.firstElementChild as HTMLElement).style.maskImage;
  expect(mask.startsWith('url("')).toBe(true);
  expect(mask.endsWith('")')).toBe(true);
});
