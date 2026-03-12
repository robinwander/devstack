// @vitest-environment jsdom

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ServiceRow } from "./service-row";

describe("ServiceRow keyboard interactions", () => {
  it("toggles row selection on Enter on the row element", () => {
    const onSelect = vi.fn();

    render(
      <ServiceRow
        name="api"
        service={{
          desired: "running",
          ready: true,
          state: "ready",
          last_failure: null,
          url: "http://localhost:3000",
        }}
        isViewing={false}
        onSelect={onSelect}
        svcColorIndex={0}
      />,
    );

    const row = screen.getByRole("button", { name: "api — ready" });
    fireEvent.keyDown(row, { key: "Enter", code: "Enter" });
    expect(onSelect).toHaveBeenCalledWith("api");
  });
});
