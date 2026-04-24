use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum WsMessage {
    Orderbook(WsOrderbook),
    Trade(WsTrade),
}

#[derive(Clone, Debug, Serialize)]
pub struct WsOrderbook {
    pub asks: Vec<WsPriceLevel>,
    pub bids: Vec<WsPriceLevel>,
    pub mid: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WsPriceLevel {
    pub price: i64,
    pub size: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct WsTrade {
    pub id: i64,
    pub price: i64,
    pub size: i64,
    pub side: String,
    pub taker: String,
    pub created_at: String,
}
