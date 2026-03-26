use crate::api::{NavigationIntent, SetNavigationIntentRequest};
use crate::app::context::AppContext;

pub async fn set_navigation_intent(
    app: &AppContext,
    request: SetNavigationIntentRequest,
) -> NavigationIntent {
    let intent = NavigationIntent {
        run_id: request.run_id,
        service: request.service,
        search: request.search,
        level: request.level,
        stream: request.stream,
        since: request.since,
        last: request.last,
        created_at: crate::util::now_rfc3339(),
    };
    app.navigation.set_intent(intent.clone()).await;
    intent
}

pub async fn clear_navigation_intent(app: &AppContext) -> bool {
    app.navigation.clear_intent().await
}
