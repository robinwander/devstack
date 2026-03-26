use crate::api::NavigationIntentResponse;
use crate::app::context::AppContext;

pub async fn get_navigation_intent(app: &AppContext) -> NavigationIntentResponse {
    NavigationIntentResponse {
        intent: app.navigation.get_intent().await,
    }
}
