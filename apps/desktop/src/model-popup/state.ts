export function modelPopupKeyAction(key: string): "close" | "ignore" {
  return key === "Escape" ? "close" : "ignore";
}
