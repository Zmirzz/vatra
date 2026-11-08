import { expect, test } from "vitest";
import { en } from "./en";
import { ro } from "./ro";
function keys(value: object, prefix = ""): string[] { return Object.entries(value).flatMap(([key, child]) => child !== null && typeof child === "object" ? keys(child, `${prefix}${key}.`) : [`${prefix}${key}`]).sort(); }
test("every key exists in English and Romanian", () => expect(keys(ro)).toEqual(keys(en)));
