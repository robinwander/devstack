// @vitest-environment jsdom

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ServiceRow } from "./service-row";

describe("ServiceRow keyboard interactions", () => {
  it("does not toggle row selection when pressing Space on an action button", () => {
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
        onRestart={vi.fn()}
        isRestarting={false}
      />,
    );

    const copyButton = screen.getByRole("button", { name: "Copy api URL" });
    fireEvent.keyDown(copyButton, { key: " ", code: "Space" });

    expect(onSelect).not.toHaveBeenCalled();
  });
});
