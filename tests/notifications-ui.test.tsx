import { StrictMode } from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import NotificationPanel, { type NotificationStatus } from "../src/settings/NotificationPanel";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

const pending: NotificationStatus = { permission: "notDetermined", canRequest: true, canSendTest: false };
const authorized: NotificationStatus = { permission: "authorized", canRequest: false, canSendTest: true };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(yes => { resolve = yes; });
  return { promise, resolve };
}

beforeEach(() => mocks.invoke.mockResolvedValue(pending));

test("opening only reads status and explicit request does not send or save preferences", async () => {
  mocks.invoke.mockImplementation(async command => command === "request_notification_permission" ? authorized : pending);
  render(<NotificationPanel disabled={false} />);
  const request = screen.getByRole("button", { name: "Request notification permission" });
  await waitFor(() => expect(request).toBeEnabled());
  expect(mocks.invoke.mock.calls.map(call => call[0])).toEqual(["get_notification_status"]);
  fireEvent.click(request);
  await screen.findByText(/macOS has authorized/);
  expect(mocks.invoke.mock.calls.map(call => call[0])).toEqual(["get_notification_status", "request_notification_permission"]);
});

test("an explicit test reports submission rather than claiming display", async () => {
  mocks.invoke.mockResolvedValue(authorized);
  render(<NotificationPanel disabled={false} />);
  const send = screen.getByRole("button", { name: "Send test notification" });
  await waitFor(() => expect(send).toBeEnabled());
  fireEvent.click(send);
  await screen.findByText(/submission does not confirm display/);
  expect(mocks.invoke.mock.calls.map(call => call[0])).toEqual(["get_notification_status", "send_test_notification"]);
});

test.each([
  ["denied", /Notifications are denied/],
  ["unavailable", /packaged macOS release app/],
  ["unknown", /unrecognized permission state/],
] as const)("%s state disables prompting and test sends", async (permission, description) => {
  mocks.invoke.mockResolvedValue({ permission, canRequest: false, canSendTest: false });
  render(<NotificationPanel disabled={false} />);
  await screen.findByText(description);
  expect(screen.getByRole("button", { name: "Request notification permission" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Send test notification" })).toBeDisabled();
});

test("quiet authorization permits a test without claiming banners are enabled", async () => {
  mocks.invoke.mockResolvedValue({ permission: "provisional", canRequest: false, canSendTest: true });
  render(<NotificationPanel disabled={false} />);
  await screen.findByText(/macOS allows quiet notifications/);
  expect(screen.getByRole("button", { name: "Send test notification" })).toBeEnabled();
});

test("status errors can be refreshed without requesting permission", async () => {
  mocks.invoke.mockRejectedValueOnce({ message: "Status check failed" }).mockResolvedValue(authorized);
  render(<NotificationPanel disabled={false} />);
  await screen.findByText("Status check failed");
  fireEvent.click(screen.getByRole("button", { name: "Refresh notification status" }));
  await screen.findByText(/macOS has authorized/);
  expect(mocks.invoke.mock.calls.every(call => call[0] === "get_notification_status")).toBe(true);
});

test("pending requests block duplicate operations and report denial", async () => {
  const response = deferred<NotificationStatus>();
  mocks.invoke.mockImplementation(command => command === "get_notification_status" ? Promise.resolve(pending) : response.promise);
  render(<NotificationPanel disabled={false} />);
  const request = screen.getByRole("button", { name: "Request notification permission" });
  await waitFor(() => expect(request).toBeEnabled());
  fireEvent.click(request);
  fireEvent.click(request);
  expect(mocks.invoke.mock.calls.filter(call => call[0] === "request_notification_permission")).toHaveLength(1);
  expect(screen.getByRole("button", { name: "Refresh notification status" })).toBeDisabled();
  await act(async () => response.resolve({ permission: "denied", canRequest: false, canSendTest: false }));
  await screen.findByText(/Notifications are denied/);
});

test("a failed or revoked-permission submission stays visible without automatic retries", async () => {
  mocks.invoke.mockImplementation(async command => {
    if (command === "send_test_notification") throw { message: "Permission was revoked" };
    return authorized;
  });
  render(<NotificationPanel disabled={false} />);
  const send = screen.getByRole("button", { name: "Send test notification" });
  await waitFor(() => expect(send).toBeEnabled());
  fireEvent.click(send);
  await screen.findByText("Permission was revoked");
  expect(mocks.invoke.mock.calls.filter(call => call[0] === "send_test_notification")).toHaveLength(1);
  expect(screen.queryByText(/submission does not confirm display/)).not.toBeInTheDocument();
});

test("StrictMode discards an older permission response", async () => {
  const first = deferred<NotificationStatus>();
  const second = deferred<NotificationStatus>();
  mocks.invoke.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
  render(<StrictMode><NotificationPanel disabled={false} /></StrictMode>);
  await act(async () => second.resolve(authorized));
  await screen.findByText(/macOS has authorized/);
  await act(async () => first.resolve({ permission: "denied", canRequest: false, canSendTest: false }));
  expect(screen.getByRole("button", { name: "Send test notification" })).toBeEnabled();
});

test("notification controls are disabled while settings are saving", async () => {
  mocks.invoke.mockResolvedValue(authorized);
  render(<NotificationPanel disabled />);
  await screen.findByText(/macOS has authorized/);
  expect(screen.getByRole("button", { name: "Send test notification" })).toBeDisabled();
});
