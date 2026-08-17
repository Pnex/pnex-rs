//! Pages de l'app. Chaque page est un composant routé (cf. `app::Route`) ;
//! `shell::Shell` est le layout racine (sidebar + garde de session).

pub mod auth_callback;
pub mod catalog;
pub mod dashboard;
pub mod devices;
pub mod login;
pub mod not_found;
pub mod orgs;
pub mod profile;
pub mod server_url;
pub mod shell;

pub use auth_callback::AuthCallback;
pub use catalog::Catalog;
pub use dashboard::Dashboard;
pub use devices::Devices;
pub use not_found::NotFound;
pub use orgs::Orgs;
pub use profile::Profile;
