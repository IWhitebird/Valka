use tonic::transport::Channel;
use valka_proto::api_service_client::ApiServiceClient;
use valka_proto::*;

use crate::error::SdkError;

/// Client for interacting with the Valka API (task CRUD operations).
#[derive(Clone)]
pub struct ValkaClient {
    inner: ApiServiceClient<Channel>,
}

impl ValkaClient {
    pub async fn connect(addr: &str) -> Result<Self, SdkError> {
        let channel = Channel::from_shared(addr.to_string())
            .map_err(|e| SdkError::Connection(e.to_string()))?
            .connect()
            .await?;

        Ok(Self {
            inner: ApiServiceClient::new(channel),
        })
    }

    pub async fn create_task(
        &mut self,
        queue_name: &str,
        task_name: &str,
        input: Option<serde_json::Value>,
    ) -> Result<TaskMeta, SdkError> {
        let response = self
            .inner
            .create_task(CreateTaskRequest {
                queue_name: queue_name.to_string(),
                task_name: task_name.to_string(),
                input: input.map(|v| v.to_string()).unwrap_or_default(),
                priority: 0,
                max_retries: 3,
                timeout_seconds: 300,
                idempotency_key: String::new(),
                metadata: String::new(),
                scheduled_at: String::new(),
            })
            .await?;

        response
            .into_inner()
            .task
            .ok_or_else(|| SdkError::Handler("No task in response".to_string()))
    }

    pub async fn get_task(&mut self, task_id: &str) -> Result<TaskMeta, SdkError> {
        let response = self
            .inner
            .get_task(GetTaskRequest {
                task_id: task_id.to_string(),
            })
            .await?;

        response
            .into_inner()
            .task
            .ok_or_else(|| SdkError::Handler("No task in response".to_string()))
    }

    pub async fn list_tasks(
        &mut self,
        queue_name: Option<&str>,
        status: Option<i32>,
        page_size: i32,
    ) -> Result<Vec<TaskMeta>, SdkError> {
        let response = self
            .inner
            .list_tasks(ListTasksRequest {
                queue_name: queue_name.unwrap_or_default().to_string(),
                status: status.unwrap_or(0),
                pagination: Some(Pagination {
                    page_size,
                    page_token: String::new(),
                }),
            })
            .await?;

        Ok(response.into_inner().tasks)
    }

    pub async fn cancel_task(&mut self, task_id: &str) -> Result<TaskMeta, SdkError> {
        let response = self
            .inner
            .cancel_task(CancelTaskRequest {
                task_id: task_id.to_string(),
            })
            .await?;

        response
            .into_inner()
            .task
            .ok_or_else(|| SdkError::Handler("No task in response".to_string()))
    }
}
