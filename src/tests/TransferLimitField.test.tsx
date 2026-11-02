// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TransferLimitField } from "../components/TransferLimitField";
afterEach(cleanup);
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
});
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
  await user.type(input, "-1");
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

test("large limits and unlimited transfers are accepted", async () => {
  const user = userEvent.setup();
  const save = vi.fn().mockResolvedValue(true);
  render(
    <TransferLimitField
      label="文件上限"
      value={128}
      disabled={false}
      onSave={save}
    />,
  );
  const input = screen.getByRole("spinbutton", { name: "文件上限" });
  await user.clear(input);
  await user.type(input, "4096");
  await user.keyboard("{Enter}");
  await waitFor(() => expect(save).toHaveBeenCalledWith(4096));
  await user.click(screen.getByRole("combobox", { name: "文件上限单位" }));
  await user.click(screen.getByRole("option", { name: "GiB" }));
  await user.clear(input);
  await user.type(input, "2");
  await user.keyboard("{Enter}");
  await waitFor(() => expect(save).toHaveBeenCalledWith(2048));
  await user.clear(input);
  await user.type(input, "0");
  await user.keyboard("{Enter}");
  await waitFor(() => expect(save).toHaveBeenCalledWith(0));
});
