export type Shipment =
  | { state: "waiting" }
  | { state: "shipped"; code: string };
