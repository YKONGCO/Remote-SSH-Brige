// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TransferLimitField } from "./TransferLimitField";
afterEach(cleanup);
test("custom limits save on Enter and invalid limits never reach the backend", async () => {
  const user = userEvent.setup();
  const save = vi.fn().mockResolvedValue(true);
  render(
    <TransferLimitField
      label="上传上限"
      value={128}
      disabled={false}
      onSave={save}
    />,
  );
  const input = screen.getByRole("spinbutton", { name: "上传上限" });
  await user.clear(input);
  await user.type(input, "237");
  expect(save).not.toHaveBeenCalled();
  await user.keyboard("{Enter}");
  await waitFor(() => expect(save).toHaveBeenCalledWith(237));
  save.mockClear();
  await user.clear(input);
  await user.type(input, "257");
  await user.tab();
  await screen.findByRole("alert");
  expect(save).not.toHaveBeenCalled();
});
test("failed saves restore the persisted limit", async () => {
  const user = userEvent.setup();
  const save = vi.fn().mockResolvedValue(false);
  render(
    <TransferLimitField
      label="下载上限"
      value={128}
      disabled={false}
      onSave={save}
    />,
  );
  const input = screen.getByRole("spinbutton", { name: "下载上限" });
  await user.clear(input);
  await user.type(input, "200");
  await user.tab();
  await waitFor(() => expect((input as HTMLInputElement).value).toBe("128"));
});
