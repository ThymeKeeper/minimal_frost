use crate::results::ResultsContent;
use crate::tile_rowstore::TileRowStore;
use odbc::{create_environment_v3, Statement, ResultSetState, Data, Handle};
use odbc::ffi::{SQLCancel, SQLHSTMT};
use std::{
    sync::{Arc, Mutex},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub struct SafeStmt(SQLHSTMT);
unsafe impl Send for SafeStmt {}
unsafe impl Sync for SafeStmt {}

#[derive(Debug)]
pub enum DbWorkerRequest {
    RunQueries(Vec<(String, String)>), // (query, context)
    Cancel,
    Quit,
}

#[derive(Debug)]
pub enum DbWorkerResponse {
    Connected,
    QueryStarted { query_idx: usize, started: Instant, query_context: String },
    QueryFinished { query_idx: usize, elapsed: Duration, result: ResultsContent },
    QueryError { query_idx: usize, elapsed: Duration, message: String },
}

pub fn start_db_worker(
    conn_str: String,
) -> (
    Sender<DbWorkerRequest>,
    Receiver<DbWorkerResponse>,
    Arc<Mutex<Option<SafeStmt>>>,
) {
    let (req_tx, req_rx) = mpsc::channel();
    let (resp_tx, resp_rx) = mpsc::channel();
    
    let current_stmt: Arc<Mutex<Option<SafeStmt>>> = Arc::new(Mutex::new(None));
    let thread_stmt = Arc::clone(&current_stmt);
    
    thread::spawn(move || {
        // Try to create environment
        let env = match create_environment_v3() {
            Ok(env) => env,
            Err(_) => {
                // Keep thread alive but not connected
                loop {
                    match req_rx.recv() {
                        Ok(DbWorkerRequest::Quit) | Err(_) => break,
                        _ => continue,
                    }
                }
                return;
            }
        };
        
        // Try to connect
        let conn = match env.connect_with_connection_string(&conn_str) {
            Ok(conn) => {
                // Signal successful connection
                let _ = resp_tx.send(DbWorkerResponse::Connected);
                
                // Enable all secondary roles by default
                if let Ok(stmt) = Statement::with_parent(&conn) {
                    let _ = stmt.exec_direct("USE SECONDARY ROLES ALL");
                }
                
                conn
            }
            Err(e) => {
                // Keep thread alive but not connected
                loop {
                    match req_rx.recv() {
                        Ok(DbWorkerRequest::Quit) | Err(_) => break,
                        _ => continue,
                    }
                }
                return;
            }
        };
        
        // Main worker loop
        loop {
            match req_rx.recv() {
                Ok(DbWorkerRequest::RunQueries(queries)) => {
                    for (idx, (query, context)) in queries.into_iter().enumerate() {
                        let started = Instant::now();
                        
                        // Send query started notification
                        let _ = resp_tx.send(DbWorkerResponse::QueryStarted {
                            query_idx: idx,
                            started,
                            query_context: context.clone(),
                        });
                        
                        // Execute query
                        match Statement::with_parent(&conn) {
                            Ok(mut stmt) => {
                                // Store statement handle for cancellation
                                unsafe {
                                    let mut current = thread_stmt.lock().unwrap();
                                    *current = Some(SafeStmt(stmt.handle()));
                                }
                                
                                match stmt.exec_direct(&query) {
                                    Ok(ResultSetState::Data(mut statement)) => {
                                        // Collect column headers
                                        let num_cols = match statement.num_result_cols() {
                                            Ok(n) => n,
                                            Err(e) => {
                                                let _ = resp_tx.send(DbWorkerResponse::QueryError {
                                                    query_idx: idx,
                                                    elapsed: started.elapsed(),
                                                    message: format!("Failed to get column count: {:?}", e),
                                                });
                                                continue;
                                            }
                                        };
                                        
                                        let mut col_names = Vec::with_capacity(num_cols as usize);
                                        for i in 1..=num_cols {
                                            match statement.describe_col(i as u16) {
                                                Ok(desc) => col_names.push(desc.name),
                                                Err(e) => {
                                                    let _ = resp_tx.send(DbWorkerResponse::QueryError {
                                                        query_idx: idx,
                                                        elapsed: started.elapsed(),
                                                        message: format!("Failed to get column name: {:?}", e),
                                                    });
                                                    continue;
                                                }
                                            }
                                        }
                                        
                                        // Create tile store from results
                                        let tile_store = match TileRowStore::from_rows(
                                            &col_names,
                                            std::iter::from_fn(|| {
                                                match statement.fetch() {
                                                    Ok(Some(mut cursor)) => {
                                                        let mut row = Vec::with_capacity(col_names.len());
                                                        for idx in 0..col_names.len() {
                                                            let val: Option<String> = cursor.get_data(idx as u16 + 1).unwrap_or(None);
                                                            row.push(val.unwrap_or_else(|| crate::tile_rowstore::NULL_SENTINEL.to_string()));
                                                        }
                                                        Some(row)
                                                    }
                                                    _ => None
                                                }
                                            })
                                        ) {
                                            Ok(store) => store,
                                            Err(e) => {
                                                let _ = resp_tx.send(DbWorkerResponse::QueryError {
                                                    query_idx: idx,
                                                    elapsed: started.elapsed(),
                                                    message: format!("Failed to create tile store: {:?}", e),
                                                });
                                                continue;
                                            }
                                        };
                                        
                                        let _ = resp_tx.send(DbWorkerResponse::QueryFinished {
                                            query_idx: idx,
                                            elapsed: started.elapsed(),
                                            result: ResultsContent::Table {
                                                headers: col_names,
                                                tile_store,
                                            },
                                        });
                                    }
                                    Ok(ResultSetState::NoData(statement)) => {
                                        let msg = if let Ok(cnt) = statement.affected_row_count() {
                                            if cnt > 0 {
                                                format!("Statement affected {} row{}", cnt, if cnt == 1 { "" } else { "s" })
                                            } else if cnt == 0 {
                                                "Statement executed successfully (no rows affected).".to_string()
                                            } else {
                                                "Statement executed successfully.".to_string()
                                            }
                                        } else {
                                            "Statement executed successfully.".to_string()
                                        };
                                        
                                        let _ = resp_tx.send(DbWorkerResponse::QueryFinished {
                                            query_idx: idx,
                                            elapsed: started.elapsed(),
                                            result: ResultsContent::Info { message: msg },
                                        });
                                    }
                                    Err(e) => {
                                        let _ = resp_tx.send(DbWorkerResponse::QueryError {
                                            query_idx: idx,
                                            elapsed: started.elapsed(),
                                            message: format!("Query execution failed: {:?}", e),
                                        });
                                    }
                                }
                                
                                // Clear statement handle
                                {
                                    let mut current = thread_stmt.lock().unwrap();
                                    *current = None;
                                }
                            }
                            Err(e) => {
                                let _ = resp_tx.send(DbWorkerResponse::QueryError {
                                    query_idx: idx,
                                    elapsed: started.elapsed(),
                                    message: format!("Failed to create statement: {}", e),
                                });
                            }
                        }
                    }
                }
                Ok(DbWorkerRequest::Cancel) => {
                    // Cancel current statement if any
                    let current = thread_stmt.lock().unwrap();
                    if let Some(SafeStmt(handle)) = *current {
                        unsafe {
                            let _ = SQLCancel(handle);
                        }
                    }
                }
                Ok(DbWorkerRequest::Quit) | Err(_) => break,
            }
        }
    });
    
    (req_tx, resp_rx, current_stmt)
}