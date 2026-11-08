import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { ExtensionNotice } from "./ExtensionNotice";
test("limited extensions produce one summary with details available on demand", async () => {
  render(<ExtensionNotice reasons={["Skill A cannot run", "Tool B needs a value"]} />);
  expect(screen.getAllByRole("status")).toHaveLength(1);
  expect(screen.queryByText("Skill A cannot run")).toBeNull();
  await userEvent.click(screen.getByText("Details"));
  expect(screen.getByText("Skill A cannot run")).toBeTruthy();
  expect(screen.getByText("Tool B needs a value")).toBeTruthy();
});
