<div align="center">

<img src="assets/logo.png" alt="ordr.trade" width="160" />

# ordr-backend

Indexer and API for [ordr.trade](https://ordr.trade) - the fully on-chain CLOB on Solana.

[Website](https://ordr.trade) &nbsp;&middot;&nbsp; [X / Twitter](https://x.com/ordrtrade)

[![Rust CI](https://github.com/ordrdottrade/ordr-backend/actions/workflows/ci.yml/badge.svg)](https://github.com/ordrdottrade/ordr-backend/actions/workflows/ci.yml)
![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)
![Solana](https://img.shields.io/badge/Solana-devnet-9945FF?style=flat-square)

</div>

## Overview

The on-chain program handles settlement, but someone has to watch the chain, maintain a global view of the orderbook, and route taker orders to the best available makers. That is what this backend does.

It polls Solana devnet, indexes all maker market accounts and their critbit slabs into Postgres, and exposes a REST API. When a taker submits an order, the backend finds the best fills, constructs the unsigned `match_taker_order` instruction with all accounts resolved, and returns a base64-serialized transaction for the frontend to sign and submit.

Part of the ordr ecosystem - see [ordr](https://github.com/ordrdottrade/ordr) for the on-chain program this backend indexes.

Built by Chaos Labs.

<div align="center">

[@4rjunc](https://x.com/4rjunc) &nbsp;&middot;&nbsp; [@avhidotsol](https://x.com/avhidotsol) &nbsp;&middot;&nbsp; [@boomheadvt](https://x.com/boomheadvt) &nbsp;&middot;&nbsp; [@Vinayapr23](https://x.com/Vinayapr23)

</div>

## API

| Method | Route          | Description                                            |
| ------ | -------------- | ------------------------------------------------------ |
| `GET`  | `/health`      | DB health check                                        |
| `GET`  | `/makers`      | List all indexed markets                               |
| `GET`  | `/orderbook`   | Current order book (12 levels each side)               |
| `GET`  | `/orders`      | List resting orders, filterable by owner               |
| `POST` | `/match_order` | Match a taker order, returns unsigned transaction      |
| `GET`  | `/trades`      | Recent trades, filterable by taker                     |
| `POST` | `/trades`      | Record a settled trade (called internally after match) |

### `GET /orderbook`

Response:

```json
{
  "asks": [{ "price": 86500000, "size": 2000000 }],
  "bids": [{ "price": 86490000, "size": 2000000 }],
  "mid": 86495000
}
```

Prices and sizes are in raw on-chain units (price = ticks x tick_size, size = lots x lot_size).

### `GET /orders`

Query params:

- `owner` - optional wallet pubkey to filter by maker
- `history` - set to `"true"` to include filled and cancelled orders (default: open only)

Response: array of order objects with `order_id`, `side`, `offset`, `size`, `filled_size`, `status`, `mid_price`, `tick_size`.

### `POST /match_order`

Request:

```json
{
  "side": "bid",
  "size": 200,
  "limit_price": 160,
  "taker": "<wallet pubkey>",
  "taker_base_ata": "<base token account>",
  "taker_quote_ata": "<quote token account>"
}
```

- `side` - `"bid"` (buying base) or `"ask"` (selling base)
- `size` - amount in base token units
- `limit_price` - optional. max price for bids, min price for asks. omit for market order

Response:

```json
{
  "transaction": "<base64 unsigned transaction>"
}
```

The frontend decodes this, signs with the taker's wallet, and submits to Solana.

### `GET /trades`

Query params:

- `taker` - optional wallet pubkey. returns that taker's last 100 trades. omit for 50 most recent across all takers.

## Related repos

- [ordr](https://github.com/ordrdottrade/ordr) - on-chain program (canonical entrypoint)
- [ordr-market-maker](https://github.com/ordrdottrade/ordr-market-maker) - MM bot that places and reprices orders
- [ordr-frontend](https://github.com/ordrdottrade/ordr-frontend) - Next.js trading UI
