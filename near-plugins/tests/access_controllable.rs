mod common;

use common::access_controllable_contract::{AccessControllableContract, Caller};
use common::utils::{assert_insufficient_acl_permissions, assert_private_method_failure};
use near_sdk::serde_json::json;
use workspaces::network::Sandbox;
use workspaces::result::ExecutionFinalResult;
use workspaces::{Account, Contract, Worker};

const PROJECT_PATH: &str = "./tests/contracts/access_controllable";

/// Bundles resources required in tests.
struct Setup {
    /// The worker interacting with the current sandbox.
    worker: Worker<Sandbox>,
    /// Deployed instance of the contract in [`PROJECT_PATH`].
    contract: AccessControllableContract,
    /// A newly created account (which differs from the contract).
    account: Account,
}

impl Setup {
    async fn new() -> anyhow::Result<Self> {
        let worker = workspaces::sandbox().await?;
        let wasm = workspaces::compile_project(PROJECT_PATH).await?;
        let contract = AccessControllableContract::new(worker.dev_deploy(&wasm).await?);
        let account = worker.dev_create_account().await?;

        Ok(Self {
            worker,
            contract,
            account,
        })
    }

    async fn new_account_with_roles(&self, roles: &[&str]) -> anyhow::Result<Account> {
        let account = self.worker.dev_create_account().await?;
        for &role in roles {
            self.contract
                .acl_grant_role_unchecked(Caller::Contract, role, account.id())
                .await?
                .into_result()?;
        }
        Ok(account)
    }
}

/// Represents the outcome of a transaction sent to the [`PROJECT_PATH`]
/// contract.
// TODO generic `T` instead of `String`
#[derive(Debug)]
enum TxOutcome {
    Success(String),
    AclFailure(AclFailure),
}

#[derive(Debug)]
struct AclFailure {
    method_name: String,
    /// The roles that are allowed (specified in the contract).
    allowed_roles: Vec<String>,
    /// The result of the transaction. Not allowing view calls here since
    /// `ViewResultDetails` is not sufficient to verify ACL failure.
    result: ExecutionFinalResult,
}

impl TxOutcome {
    fn assert_success(&self, want: String) {
        let got = match self {
            TxOutcome::Success(got) => got.clone(),
            TxOutcome::AclFailure(failure) => panic!(
                "Expected transaction success but it failed with: {:?}",
                failure
            ),
        };
        assert_eq!(got, want);
    }

    fn assert_acl_failure(&self) {
        let failure = match self {
            TxOutcome::Success(_) => panic!("Expected transaction failure"),
            TxOutcome::AclFailure(failure) => failure,
        };
        assert_insufficient_acl_permissions(
            failure.result.clone(),
            failure.method_name.as_str(),
            failure.allowed_roles.clone(),
        );
    }
}

async fn call_restricted_greeting(
    contract: &Contract,
    caller: &Account,
) -> anyhow::Result<TxOutcome> {
    let res = caller
        .call(contract.id(), "restricted_greeting")
        .args_json(())
        .max_gas()
        .transact()
        .await?;
    let tx_outcome = match res.is_success() {
        true => TxOutcome::Success(res.into_result().unwrap().json::<String>().unwrap()),
        false => TxOutcome::AclFailure(AclFailure {
            method_name: "restricted_greeting".to_string(),
            allowed_roles: vec!["LevelA".to_string(), "LevelC".to_string()],
            result: res,
        }),
    };
    Ok(tx_outcome)
}

/// Smoke test of contract setup and basic functionality.
#[tokio::test]
async fn test_set_and_get_status() -> anyhow::Result<()> {
    let Setup {
        contract, account, ..
    } = Setup::new().await?;
    let contract = contract.contract();
    let message = "hello world";

    account
        .call(contract.id(), "set_status")
        .args_json(json!({
            "message": message,
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let res: String = account
        .call(contract.id(), "get_status")
        .args_json(json!({
            "account_id": account.id(),
        }))
        .view()
        .await?
        .json()?;

    assert_eq!(res, message);
    Ok(())
}

#[tokio::test]
async fn test_acl_has_role() -> anyhow::Result<()> {
    let Setup {
        contract, account, ..
    } = Setup::new().await?;
    let role = "LevelA";

    // Anyone may call `acl_has_role`.
    let has_role = contract
        .acl_has_role(account.clone().into(), role, account.id())
        .await?;
    assert_eq!(has_role, false);

    contract
        .acl_grant_role_unchecked(Caller::Contract, role, account.id())
        .await?
        .into_result()?;

    let has_role = contract
        .acl_has_role(account.clone().into(), role, account.id())
        .await?;
    assert_eq!(has_role, true);

    Ok(())
}

#[tokio::test]
async fn test_acl_grant_role_unchecked_is_private() -> anyhow::Result<()> {
    let Setup {
        contract, account, ..
    } = Setup::new().await?;
    let res = contract
        .acl_grant_role_unchecked(account.clone().into(), "LevelA", account.id())
        .await?;
    assert_private_method_failure(res, "acl_grant_role_unchecked");
    Ok(())
}

#[tokio::test]
async fn test_acl_grant_role_unchecked() -> anyhow::Result<()> {
    let Setup {
        contract, account, ..
    } = Setup::new().await?;
    let role = "LevelA";

    contract
        .assert_acl_has_role(false, role, account.id())
        .await;
    contract
        .acl_grant_role_unchecked(Caller::Contract, role, account.id())
        .await?
        .into_result()?;
    contract.assert_acl_has_role(true, role, account.id()).await;

    // Granting a role again doesn't lead to failures.
    contract
        .acl_grant_role_unchecked(Caller::Contract, role, account.id())
        .await?
        .into_result()?;

    Ok(())
}

#[tokio::test]
async fn test_attribute_access_control_any() -> anyhow::Result<()> {
    let setup = Setup::new().await?;
    let raw_contract = setup.contract.contract();
    let want_result = "hello world".to_string();

    // Account without any of the required permissions is restricted.
    let account = setup.new_account_with_roles(&[]).await?;
    call_restricted_greeting(raw_contract, &account)
        .await?
        .assert_acl_failure();
    let account = setup.new_account_with_roles(&["LevelB"]).await?;
    call_restricted_greeting(raw_contract, &account)
        .await?
        .assert_acl_failure();

    // Account with one of the required permissions succeeds.
    let account = setup.new_account_with_roles(&["LevelA"]).await?;
    call_restricted_greeting(raw_contract, &account)
        .await?
        .assert_success(want_result.clone());
    let account = setup.new_account_with_roles(&["LevelC"]).await?;
    call_restricted_greeting(raw_contract, &account)
        .await?
        .assert_success(want_result.clone());
    let account = setup.new_account_with_roles(&["LevelA", "LevelB"]).await?;
    call_restricted_greeting(raw_contract, &account)
        .await?
        .assert_success(want_result.clone());

    // Account with both permissions succeeds.
    let account = setup.new_account_with_roles(&["LevelA", "LevelC"]).await?;
    call_restricted_greeting(raw_contract, &account)
        .await?
        .assert_success(want_result.clone());
    let account = setup
        .new_account_with_roles(&["LevelA", "LevelB", "LevelC"])
        .await?;
    call_restricted_greeting(raw_contract, &account)
        .await?
        .assert_success(want_result.clone());

    // TODO once admin fns are implemented, add tests for cases mentioned in
    // https://github.com/aurora-is-near/near-plugins/pull/5#discussion_r973784721

    Ok(())
}
