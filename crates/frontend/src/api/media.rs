use crate::api::client::*;
use crate::types::*;
use crate::utils::cache::{cached, invalidate_cached_prefix};
use crate::utils::error::HttpError;

const MEDIA_LIST_CACHE_TTL_MS: f64 = 30_000.0;
const MEDIA_LIST_CACHE_PREFIX: &str = "media:list:";

fn media_list_cache_key(
    page: u64,
    per_page: u64,
    order_by: &str,
    order: &str,
    search_term: &str,
) -> String {
    let encoded_search = urlencoding::encode(search_term);
    format!("{MEDIA_LIST_CACHE_PREFIX}{page}:{per_page}:{order_by}:{order}:{encoded_search}")
}

pub fn invalidate_media_related_caches() {
    invalidate_cached_prefix(MEDIA_LIST_CACHE_PREFIX);
}

pub async fn get_user_media_statistics(
    page: u64,
    per_page: u64,
    order_by: &str,
    order: &str,
    search_term: &str,
) -> Result<ListResponse<UserMediaStatisticRecord>, HttpError> {
    let from = (page - 1) * per_page;
    let from_str = from.to_string();
    let limit_str = per_page.to_string();
    let dir = get_search_order(order);

    let mut params = vec![
        ("from", from_str.as_str()),
        ("limit", limit_str.as_str()),
        ("order_by", order_by),
        ("dir", dir),
    ];

    if !search_term.is_empty() {
        params.push(("search_term", search_term));
    }

    let url = build_url("/_palpo/admin/v1/statistics/users/media", &params)?;
    let response: MediaStatisticsResponse = api_client(&url, "GET", None).await?;

    Ok(ListResponse {
        data: response
            .users
            .into_iter()
            .map(|s| {
                let id = s.user_id.clone();
                UserMediaStatisticRecord { id, statistic: s }
            })
            .collect(),
        total: response.total,
    })
}

pub async fn get_user_media_statistics_cached(
    page: u64,
    per_page: u64,
    order_by: &str,
    order: &str,
    search_term: &str,
) -> Result<ListResponse<UserMediaStatisticRecord>, HttpError> {
    let cache_key = media_list_cache_key(page, per_page, order_by, order, search_term);
    cached(
        &cache_key,
        MEDIA_LIST_CACHE_TTL_MS,
        get_user_media_statistics(page, per_page, order_by, order, search_term),
    )
    .await
}

pub async fn delete_local_media(
    before_ts: u64,
    size_gt: u64,
    keep_profiles: bool,
) -> Result<DeleteMediaResult, HttpError> {
    if keep_profiles {
        super::admin_capabilities::require("media_keep_profiles").await?;
    }
    let before = before_ts.to_string();
    let size = size_gt.to_string();
    let url = build_url(
        "/_palpo/admin/v1/media/delete",
        &[
            ("before_ts", &before),
            ("size_gt", &size),
            (
                "keep_profiles",
                if keep_profiles { "true" } else { "false" },
            ),
        ],
    )?;
    let result = api_client(&url, "POST", None).await?;
    invalidate_media_related_caches();
    Ok(result)
}

#[derive(Debug, serde::Deserialize)]
pub struct PurgeMediaResult {
    pub deleted: u64,
}
pub async fn purge_remote_media(before_ts: u64) -> Result<PurgeMediaResult, HttpError> {
    let before = before_ts.to_string();
    let url = build_url(
        "/_palpo/admin/v1/purge_media_cache",
        &[("before_ts", &before)],
    )?;
    let result = api_client(&url, "POST", None).await?;
    invalidate_media_related_caches();
    Ok(result)
}
#[cfg(test)]
mod contract_tests {
    use super::*;
    #[test]
    fn purge_does_not_accept_a_default_zero_from_the_local_delete_shape() {
        assert_eq!(
            serde_json::from_str::<PurgeMediaResult>(r#"{"deleted":17}"#)
                .unwrap()
                .deleted,
            17
        );
        assert!(serde_json::from_str::<PurgeMediaResult>(r#"{"total":17}"#).is_err());
    }
}
