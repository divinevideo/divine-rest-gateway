// ABOUTME: Serializes protected creator quotas in a strongly consistent SQLite Durable Object.
use serde::Deserialize;
use worker::*;

#[derive(Deserialize, serde::Serialize)]
pub struct QuotaKeys {
    pub identity: String,
    pub ip: String,
}

#[derive(Deserialize)]
struct Count {
    bucket: String,
    count: i64,
}

struct Budget {
    bucket: String,
    window: i64,
    limit: i64,
    retry_after: i64,
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
        if [&keys.identity, &keys.ip]
            .iter()
            .any(|key| key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Response::error("invalid_quota_key", 400);
        }
        let seconds = (Date::now().as_millis() / 1000) as i64;
        let window = seconds / 60;
        // Store the final minute covered by a counter in `window`, so the same
        // indexed expiry removes both minute and UTC-day counters correctly.
        let day_end = (window / 1440 + 1) * 1440 - 1;
        let day_retry = (day_end + 1) * 60 - seconds;
        let budgets = [
            Budget {
                bucket: keys.identity.clone(),
                window,
                limit: 30,
                retry_after: 60,
            },
            Budget {
                bucket: keys.ip.clone(),
                window,
                limit: 120,
                retry_after: 60,
            },
            Budget {
                bucket: format!("daily:identity:{}", keys.identity),
                window: day_end,
                limit: 100,
                retry_after: day_retry,
            },
            Budget {
                bucket: format!("daily:ip:{}", keys.ip),
                window: day_end,
                limit: 300,
                retry_after: day_retry,
            },
            Budget {
                bucket: "daily:global".into(),
                window: day_end,
                limit: 1000,
                retry_after: day_retry,
            },
        ];
        let sql = self.state.storage().sql();
        sql.exec("CREATE TABLE IF NOT EXISTS quotas(bucket TEXT NOT NULL, window INTEGER NOT NULL, count INTEGER NOT NULL, PRIMARY KEY(bucket,window))", None)?;
        sql.exec(
            "CREATE INDEX IF NOT EXISTS quotas_window ON quotas(window)",
            None,
        )?;
        sql.exec("DELETE FROM quotas WHERE window < ?", vec![window.into()])?;
        let rows: Vec<Count> = sql
            .exec(
                "SELECT bucket,count FROM quotas WHERE (window=? AND bucket IN (?,?)) OR (window=? AND bucket IN (?,?,?))",
                vec![
                    window.into(),
                    budgets[0].bucket.clone().into(),
                    budgets[1].bucket.clone().into(),
                    day_end.into(),
                    budgets[2].bucket.clone().into(),
                    budgets[3].bucket.clone().into(),
                    budgets[4].bucket.clone().into(),
                ],
            )?
            .to_array()?;
        if let Some(retry_after) = budgets
            .iter()
            .filter(|budget| {
                rows.iter()
                    .any(|row| row.bucket == budget.bucket && row.count >= budget.limit)
            })
            .map(|budget| budget.retry_after)
            .max()
        {
            let mut response = Response::error("rate_limited", 429)?;
            response
                .headers_mut()
                .set("Retry-After", &retry_after.to_string())?;
            return Ok(response);
        }
        // There are no awaits between reading and updating. A DO cannot interleave
        // another request in this synchronous section; all five counters are
        // charged in one atomic SQLite statement, or none are charged.
        let parameters: Vec<SqlStorageValue> = budgets
            .into_iter()
            .flat_map(|budget| [budget.bucket.into(), budget.window.into()])
            .collect();
        sql.exec("INSERT INTO quotas(bucket,window,count) VALUES (?,?,1),(?,?,1),(?,?,1),(?,?,1),(?,?,1) ON CONFLICT(bucket,window) DO UPDATE SET count=count+1", parameters)?;
        Response::empty()
    }
}
