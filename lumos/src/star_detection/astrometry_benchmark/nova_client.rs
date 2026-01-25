//! Nova.astrometry.net API client.
//!
//! Provides an interface to upload images and retrieve detected star positions
//! from the nova.astrometry.net plate-solving service.

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, multipart};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

const NOVA_BASE_URL: &str = "https://nova.astrometry.net";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// A star detected by astrometry.net's source extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstrometryStar {
    /// X coordinate in pixels (1-indexed FITS convention).
    pub x: f32,
    /// Y coordinate in pixels (1-indexed FITS convention).
    pub y: f32,
    /// Estimated flux (brightness).
    pub flux: f32,
    /// Estimated background at this position.
    pub background: f32,
}

/// Job status from astrometry.net.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// Job is queued or running.
    Processing,
    /// Job completed successfully.
    Success,
    /// Job failed.
    Failure,
}

/// Client for nova.astrometry.net API.
#[derive(Debug)]
pub struct NovaClient {
    api_key: String,
    session_key: Option<String>,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    status: String,
    session: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UploadResponse {
    status: String,
    subid: Option<u64>,
    hash: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SubmissionStatusResponse {
    processing_started: Option<String>,
    processing_finished: Option<String>,
    jobs: Option<Vec<Option<u64>>>,
    job_calibrations: Option<Vec<Option<serde_json::Value>>>,
}

#[derive(Debug, Deserialize)]
struct JobStatusResponse {
    status: Option<String>,
}

impl NovaClient {
    /// Create a new Nova client with the given API key.
    ///
    /// Get your API key from https://nova.astrometry.net/api_help after logging in.
    pub fn new(api_key: String) -> Self {
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            api_key,
            session_key: None,
            client,
        }
    }

    /// Login to the API and get a session key.
    pub fn login(&mut self) -> Result<()> {
        let url = format!("{}/api/login", NOVA_BASE_URL);

        let request_json = serde_json::json!({
            "apikey": self.api_key
        });

        let response = self
            .client
            .post(&url)
            .form(&[("request-json", request_json.to_string())])
            .send()
            .context("Failed to send login request")?;

        let login_resp: LoginResponse =
            response.json().context("Failed to parse login response")?;

        if login_resp.status != "success" {
            bail!(
                "Login failed: {}",
                login_resp
                    .message
                    .unwrap_or_else(|| "Unknown error".to_string())
            );
        }

        self.session_key = login_resp.session;
        tracing::info!("Successfully logged in to nova.astrometry.net");
        Ok(())
    }

    /// Ensure we have a valid session key, logging in if necessary.
    fn ensure_session(&mut self) -> Result<String> {
        if self.session_key.is_none() {
            self.login()?;
        }
        self.session_key
            .clone()
            .context("No session key available after login")
    }

    /// Upload an image file and return the submission ID.
    pub fn upload_image(&mut self, image_path: &Path) -> Result<u64> {
        let session = self.ensure_session()?;
        let url = format!("{}/api/upload", NOVA_BASE_URL);

        let file_name = image_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Read file contents
        let file_contents = std::fs::read(image_path)
            .with_context(|| format!("Failed to read image file: {}", image_path.display()))?;

        let request_json = serde_json::json!({
            "session": session,
            "publicly_visible": "n",
            "allow_modifications": "d",
            "allow_commercial_use": "n"
        });

        // Build multipart form
        let form = multipart::Form::new()
            .text("request-json", request_json.to_string())
            .part(
                "file",
                multipart::Part::bytes(file_contents)
                    .file_name(file_name)
                    .mime_str("application/octet-stream")?,
            );

        let response = self
            .client
            .post(&url)
            .multipart(form)
            .send()
            .context("Failed to upload image")?;

        let upload_resp: UploadResponse =
            response.json().context("Failed to parse upload response")?;

        if upload_resp.status != "success" {
            bail!(
                "Upload failed: {}",
                upload_resp
                    .message
                    .unwrap_or_else(|| "Unknown error".to_string())
            );
        }

        let subid = upload_resp.subid.context("No submission ID in response")?;
        tracing::info!(
            "Uploaded image, submission ID: {}, hash: {:?}",
            subid,
            upload_resp.hash
        );

        Ok(subid)
    }

    /// Get the job IDs for a submission.
    pub fn get_submission_jobs(&self, submission_id: u64) -> Result<Vec<u64>> {
        let url = format!("{}/api/submissions/{}", NOVA_BASE_URL, submission_id);

        let response = self
            .client
            .get(&url)
            .send()
            .context("Failed to get submission status")?;

        let status: SubmissionStatusResponse = response
            .json()
            .context("Failed to parse submission status")?;

        let jobs: Vec<u64> = status
            .jobs
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect();

        Ok(jobs)
    }

    /// Check the status of a job.
    pub fn get_job_status(&self, job_id: u64) -> Result<JobStatus> {
        let url = format!("{}/api/jobs/{}", NOVA_BASE_URL, job_id);

        let response = self
            .client
            .get(&url)
            .send()
            .context("Failed to get job status")?;

        let status: JobStatusResponse = response.json().context("Failed to parse job status")?;

        match status.status.as_deref() {
            Some("success") => Ok(JobStatus::Success),
            Some("failure") => Ok(JobStatus::Failure),
            _ => Ok(JobStatus::Processing),
        }
    }

    /// Wait for a submission to complete and return the first successful job ID.
    pub fn wait_for_job(&self, submission_id: u64, timeout: Duration) -> Result<u64> {
        let start = Instant::now();

        tracing::info!(
            "Waiting for submission {} to complete (timeout: {:?})",
            submission_id,
            timeout
        );

        loop {
            if start.elapsed() > timeout {
                bail!("Timeout waiting for job to complete");
            }

            let jobs = self.get_submission_jobs(submission_id)?;

            for job_id in jobs {
                let status = self.get_job_status(job_id)?;
                match status {
                    JobStatus::Success => {
                        tracing::info!("Job {} completed successfully", job_id);
                        return Ok(job_id);
                    }
                    JobStatus::Failure => {
                        tracing::warn!("Job {} failed", job_id);
                        // Continue checking other jobs if any
                    }
                    JobStatus::Processing => {
                        tracing::debug!("Job {} still processing", job_id);
                    }
                }
            }

            sleep(POLL_INTERVAL);
        }
    }

    /// Download the .axy file (detected stars) for a job.
    ///
    /// The .axy file is a FITS BINTABLE containing the source extraction results.
    /// Returns the raw FITS data.
    pub fn download_axy(&self, job_id: u64) -> Result<Vec<u8>> {
        let url = format!("{}/axy_file/{}", NOVA_BASE_URL, job_id);

        let response = self
            .client
            .get(&url)
            .header("Referer", format!("{}/api/login", NOVA_BASE_URL))
            .send()
            .context("Failed to download axy file")?;

        if !response.status().is_success() {
            bail!("Failed to download axy file: HTTP {}", response.status());
        }

        let bytes = response.bytes().context("Failed to read axy file bytes")?;
        tracing::info!("Downloaded axy file: {} bytes", bytes.len());

        Ok(bytes.to_vec())
    }

    /// Parse the .axy FITS file to extract star positions.
    pub fn parse_axy(axy_data: &[u8]) -> Result<Vec<AstrometryStar>> {
        // Write to a temporary file since fitsio needs a file path
        let temp_dir = std::env::temp_dir();
        let temp_path = temp_dir.join(format!("axy_{}.fits", std::process::id()));
        std::fs::write(&temp_path, axy_data).context("Failed to write temp axy file")?;

        let result = Self::parse_axy_file(&temp_path);

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_path);

        result
    }

    /// Parse an .axy FITS file from disk.
    fn parse_axy_file(path: &Path) -> Result<Vec<AstrometryStar>> {
        use fitsio::FitsFile;

        let mut fptr = FitsFile::open(path).context("Failed to open axy FITS file")?;

        // The axy file has a BINTABLE extension with star data
        // Try to find the table with X, Y columns
        let hdu = fptr.hdu(1).context("No extension HDU in axy file")?;

        // Read column data
        let x_col: Vec<f32> = hdu
            .read_col(&mut fptr, "X")
            .or_else(|_| hdu.read_col(&mut fptr, "x"))
            .context("Failed to read X column")?;

        let y_col: Vec<f32> = hdu
            .read_col(&mut fptr, "Y")
            .or_else(|_| hdu.read_col(&mut fptr, "y"))
            .context("Failed to read Y column")?;

        // Flux and background are optional
        let flux_col: Vec<f32> = hdu
            .read_col(&mut fptr, "FLUX")
            .or_else(|_| hdu.read_col(&mut fptr, "flux"))
            .unwrap_or_else(|_| vec![1.0; x_col.len()]);

        let bg_col: Vec<f32> = hdu
            .read_col(&mut fptr, "BACKGROUND")
            .or_else(|_| hdu.read_col(&mut fptr, "background"))
            .unwrap_or_else(|_| vec![0.0; x_col.len()]);

        let stars: Vec<AstrometryStar> = x_col
            .into_iter()
            .zip(y_col)
            .zip(flux_col)
            .zip(bg_col)
            .map(|(((x, y), flux), background)| {
                AstrometryStar {
                    // Convert from 1-indexed FITS to 0-indexed
                    x: x - 1.0,
                    y: y - 1.0,
                    flux,
                    background,
                }
            })
            .collect();

        tracing::info!("Parsed {} stars from axy file", stars.len());
        Ok(stars)
    }

    /// Upload an image, wait for solving, and return detected stars.
    ///
    /// This is a convenience method that combines upload, wait, and download.
    pub fn solve_and_get_stars(
        &mut self,
        image_path: &Path,
        timeout: Duration,
    ) -> Result<Vec<AstrometryStar>> {
        let submission_id = self.upload_image(image_path)?;
        let job_id = self.wait_for_job(submission_id, timeout)?;
        let axy_data = self.download_axy(job_id)?;
        Self::parse_axy(&axy_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nova_client_creation() {
        let client = NovaClient::new("test_api_key".to_string());
        assert!(client.session_key.is_none());
    }

    #[test]
    #[ignore] // Requires API key
    fn test_login() {
        let api_key = std::env::var("NOVA_API_KEY").expect("NOVA_API_KEY not set");
        let mut client = NovaClient::new(api_key);
        client.login().expect("Login failed");
        assert!(client.session_key.is_some());
    }
}
