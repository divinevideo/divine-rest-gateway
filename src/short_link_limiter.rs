// ABOUTME: Serializes anonymous creator IP quotas in a strongly consistent SQLite Durable Object.
use serde::Deserialize;
use worker::*;

#[derive(Deserialize, serde::Serialize)]
pub struct QuotaKeys {
    pub ip: String,
}

#[derive(Deserialize)]
struct Count {
    count: i64,
}

#[durable_object]
pub struct ShortLinkLimiter {
    state: State,
}

impl DurableObject for ShortLinkLimiter {
    fn new(state: State, _env: Env) -> Self {
        Self { state }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if req.method() != Method::Post {
            return Response::error("method_not_allowed", 405);
        }
        let keys: QuotaKeys = req.json().await?;
        if keys.ip.len() != 64 || !keys.ip.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Response::error("invalid_quota_key", 400);
        }
        let window = (Date::now().as_millis() / 60000) as i64;
        let sql = self.state.storage().sql();
        sql.exec("CREATE TABLE IF NOT EXISTS quotas(bucket TEXT NOT NULL, window INTEGER NOT NULL, count INTEGER NOT NULL, PRIMARY KEY(bucket,window))", None)?;
        sql.exec(
            "DELETE FROM quotas WHERE window < ?",
            vec![(window - 4).into()],
        )?;
        let rows: Vec<Count> = sql
            .exec(
                "SELECT count FROM quotas WHERE window=? AND bucket=?",
                vec![window.into(), keys.ip.clone().into()],
            )?
            .to_array()?;
        if rows.iter().any(|row| row.count >= 120) {
            let mut response = Response::error("rate_limited", 429)?;
            response.headers_mut().set("Retry-After", "60")?;
            return Ok(response);
        }
        // There are no awaits between reading and updating. A DO cannot interleave
        // another request in this synchronous section; the counter write is atomic.
        sql.exec("INSERT INTO quotas(bucket,window,count) VALUES (?,?,1) ON CONFLICT(bucket,window) DO UPDATE SET count=count+1", vec![keys.ip.into(), window.into()])?;
        Response::empty()
    }
}
