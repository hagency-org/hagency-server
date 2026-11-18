use crate::api::client::*;
use crate::types::*;
use crate::utils::error::HttpError;

pub async fn get_destinations(
    page: u64,
    per_page: u64,
    order_by: &str,
    order: &str,
    search: &str,
) -> Result<ListResponse<DestinationRecord>, HttpError> {
    if !search.is_empty() {
        super::admin_capabilities::require("destination_search").await?;
    }
    let from = (page - 1) * per_page;
    let from_str = from.to_string();
    let limit_str = per_page.to_string();
    let dir = get_search_order(order);

    let url = build_url(
        "/_palpo/admin/v1/federation/destinations",
        &[
            ("from", &from_str),
            ("limit", &limit_str),
            ("order_by", order_by),
            ("dir", dir),
            ("destination", search),
        ],
    )?;

    let response: DestinationsResponse = api_client(&url, "GET", None).await?;

    Ok(ListResponse {
        data: response
            .destinations
            .into_iter()
            .map(|d| {
                let id = d.destination.clone();
                DestinationRecord { id, destination: d }
            })
            .collect(),
        total: response.total,
    })
}

pub async fn get_destination(id: &str) -> Result<DestinationRecord, HttpError> {
    let encoded = urlencoding::encode(id);
    let url = build_url(
        &format!("/_palpo/admin/v1/federation/destinations/{encoded}"),
        &[],
    )?;
    let dest: Destination = api_client(&url, "GET", None).await?;
    Ok(DestinationRecord {
        id: dest.destination.clone(),
        destination: dest,
    })
}

pub async fn reset_destination_connection(id: &str) -> Result<(), HttpError> {
    let encoded = urlencoding::encode(id);
    let url = build_url(
        &format!("/_palpo/admin/v1/federation/destinations/{encoded}/reset_connection"),
        &[],
    )?;
    let _: serde_json::Value = api_client(&url, "POST", None).await?;
    Ok(())
}
